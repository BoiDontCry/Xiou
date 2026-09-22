[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$expectedPowerShellVersion = '7.6.6'
$expectedRustVersion = '1.97.1'
$expectedRustHost = 'x86_64-pc-windows-msvc'
$expectedCargoVersion = '1.97.1'
$expectedMsvcToolsVersion = '14.44.35207'
$expectedLinkVersion = '14.44.35228.0'
$expectedWindowsSdkVersion = '10.0.26100.0'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$distRoot = Join-Path $repositoryRoot 'dist'
$manifestPath = Join-Path $repositoryRoot 'Cargo.toml'
$readmePath = Join-Path $repositoryRoot 'README-使用说明.md'
$originalEnvironment = [Environment]::GetEnvironmentVariables('Process')

Push-Location $repositoryRoot
try {
    $actualPowerShellVersion = $PSVersionTable.PSVersion.ToString()
    if ($actualPowerShellVersion -ne $expectedPowerShellVersion) {
        throw "需要 PowerShell $expectedPowerShellVersion，当前为 $actualPowerShellVersion。"
    }

    $rustcPath = (Get-Command rustc -CommandType Application -ErrorAction Stop).Source
    $rustVersion = & $rustcPath -vV
    if ($LASTEXITCODE -ne 0) {
        throw '无法读取 rustc 版本。'
    }
    $actualRustVersion = ($rustVersion | Where-Object { $_ -like 'release: *' }) -replace '^release: ', ''
    $actualRustHost = ($rustVersion | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    if ($actualRustVersion -ne $expectedRustVersion -or $actualRustHost -ne $expectedRustHost) {
        throw "需要 rustc $expectedRustVersion ($expectedRustHost)，当前为 $actualRustVersion ($actualRustHost)。"
    }
    $env:RUSTC = $rustcPath
    $env:CARGO_BUILD_RUSTC = $rustcPath
    Remove-Item Env:RUSTC_WRAPPER, Env:RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_BUILD_RUSTC_WRAPPER, Env:CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue

    $cargoPath = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
    $cargoVersion = & $cargoPath -Vv
    if ($LASTEXITCODE -ne 0) {
        throw '无法读取 Cargo 版本。'
    }
    $actualCargoVersion = ($cargoVersion | Where-Object { $_ -like 'release: *' }) -replace '^release: ', ''
    $actualCargoHost = ($cargoVersion | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    if ($actualCargoVersion -ne $expectedCargoVersion -or $actualCargoHost -ne $expectedRustHost) {
        throw "需要 Cargo $expectedCargoVersion ($expectedRustHost)，当前为 $actualCargoVersion ($actualCargoHost)。"
    }

    $programFilesX86 = ${env:ProgramFiles(x86)}
    $vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw "未找到 Visual Studio Installer：$vswhere"
    }
    $vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($vsInstall)) {
        throw '未找到 Visual Studio C++ Build Tools。'
    }

    $vsDevCmd = Join-Path $vsInstall 'Common7\Tools\VsDevCmd.bat'
    if (-not (Test-Path -LiteralPath $vsDevCmd)) {
        throw "未找到 Visual Studio 开发环境脚本：$vsDevCmd"
    }
    $devCommand = "`"$vsDevCmd`" -no_logo -arch=x64 -host_arch=x64 -winsdk=$expectedWindowsSdkVersion -vcvars_ver=$expectedMsvcToolsVersion >nul && set"
    $devEnvironment = & $env:ComSpec /d /s /c $devCommand
    if ($LASTEXITCODE -ne 0) {
        throw '无法初始化指定的 MSVC 和 Windows SDK 环境。'
    }
    foreach ($line in $devEnvironment) {
        $separator = $line.IndexOf('=')
        if ($separator -gt 0) {
            [Environment]::SetEnvironmentVariable(
                $line.Substring(0, $separator),
                $line.Substring($separator + 1),
                'Process'
            )
        }
    }
    if ($env:VCToolsVersion -ne $expectedMsvcToolsVersion -or
        $env:WindowsSDKVersion.TrimEnd('\') -ne $expectedWindowsSdkVersion) {
        throw "开发环境未选择指定的 MSVC $expectedMsvcToolsVersion 和 Windows SDK $expectedWindowsSdkVersion。"
    }

    $linker = Join-Path $vsInstall "VC\Tools\MSVC\$expectedMsvcToolsVersion\bin\Hostx64\x64\link.exe"
    if (-not (Test-Path -LiteralPath $linker)) {
        throw "未找到指定的 MSVC linker：$linker"
    }
    $actualLinkVersion = (Get-Item -LiteralPath $linker).VersionInfo.FileVersion
    if ($actualLinkVersion -ne $expectedLinkVersion) {
        throw "需要 link.exe $expectedLinkVersion，当前为 $actualLinkVersion。"
    }

    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $linker

    $env:CARGO_TARGET_DIR = Join-Path $repositoryRoot 'target'
    Remove-Item Env:CARGO_BUILD_TARGET -ErrorAction SilentlyContinue
    foreach ($name in [Environment]::GetEnvironmentVariables('Process').Keys) {
        if ($name -like 'CARGO_PROFILE_RELEASE_*') {
            Remove-Item -LiteralPath "Env:$name"
        }
        if ($name -match '^(CC|CFLAGS|CXX|CXXFLAGS|AR|ARFLAGS|RANLIB)(_|$)' -or
            $name -match '^(HOST|TARGET)_(CC|CFLAGS|CXX|CXXFLAGS|AR|ARFLAGS|RANLIB)$' -or
            $name -in @(
                'CRATE_CC_NO_DEFAULTS'
                'CC_SHELL_ESCAPED_FLAGS'
                'CL'
                '_CL_'
                'LINK'
                '_LINK_'
                'LIBSQLITE3_FLAGS'
                'LIBSQLITE3_SYS_BUNDLING'
                'LIBSQLITE3_SYS_USE_PKG_CONFIG'
                'SQLITE_MAX_COLUMN'
                'SQLITE_MAX_EXPR_DEPTH'
                'SQLITE_MAX_VARIABLE_NUMBER'
            )) {
            Remove-Item -LiteralPath "Env:$name"
        }
    }

    $metadataJson = & $cargoPath metadata --offline --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata 失败。'
    }
    $metadata = $metadataJson | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.manifest_path -eq $manifestPath }
    if ($null -eq $package) {
        throw '未找到当前 Cargo 包。'
    }

    $packageName = "xiou-$($package.version)-windows-x64"
    $stageRoot = Join-Path $distRoot $packageName
    $zipPath = Join-Path $distRoot "$packageName.zip"
    $zipHashPath = "$zipPath.sha256"

    New-Item -ItemType Directory -Force $distRoot | Out-Null
    if (Test-Path -LiteralPath $stageRoot) {
        Remove-Item -Recurse -Force -LiteralPath $stageRoot
    }
    Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $zipPath, $zipHashPath
    New-Item -ItemType Directory $stageRoot | Out-Null

    $remapFlags = @(
        "--remap-path-prefix=$repositoryRoot=."
        "--remap-path-prefix=$env:USERPROFILE=~"
        '-C'
        'target-feature=+crt-static'
        '-C'
        'link-arg=/Brepro'
    )
    Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    $env:CARGO_ENCODED_RUSTFLAGS = $remapFlags -join [char]0x1f
    $releaseProfileConfig = @(
        '--config'
        'profile.release.opt-level=3'
        '--config'
        'profile.release.debug=false'
        '--config'
        'profile.release.split-debuginfo="off"'
        '--config'
        'profile.release.debug-assertions=false'
        '--config'
        'profile.release.overflow-checks=false'
        '--config'
        'profile.release.lto="thin"'
        '--config'
        'profile.release.panic="unwind"'
        '--config'
        'profile.release.incremental=false'
        '--config'
        'profile.release.codegen-units=1'
        '--config'
        'profile.release.strip=true'
        '--config'
        'profile.release.rpath=false'
    )

    $buildMessages = @(
        & $cargoPath build @releaseProfileConfig --offline --locked --release --bin xiou --target $expectedRustHost --message-format=json-render-diagnostics |
            ForEach-Object { $_ | ConvertFrom-Json }
    )
    if ($LASTEXITCODE -ne 0) {
        throw 'Release 构建失败。'
    }

    $releaseExe = $buildMessages |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'xiou' -and $null -ne $_.executable } |
        Select-Object -Last 1 -ExpandProperty executable
    if ([string]::IsNullOrWhiteSpace($releaseExe) -or -not (Test-Path -LiteralPath $releaseExe)) {
        throw 'Cargo 未返回 xiou 的实际产物路径。'
    }
    $stagedExe = Join-Path $stageRoot 'xiou.exe'
    Copy-Item -LiteralPath $releaseExe -Destination $stagedExe
    Copy-Item -LiteralPath $readmePath -Destination $stageRoot

    $exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedExe).Hash
    "$exeHash  xiou.exe" | Set-Content -Encoding utf8 (Join-Path $stageRoot 'SHA256SUMS.txt')
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archiveTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
    $archive = [IO.Compression.ZipFile]::Open($zipPath, [IO.Compression.ZipArchiveMode]::Create)
    try {
        Get-ChildItem -LiteralPath $stageRoot -File | Sort-Object Name | ForEach-Object {
            $entry = $archive.CreateEntry(
                "$packageName/$($_.Name)",
                [IO.Compression.CompressionLevel]::Optimal
            )
            $entry.LastWriteTime = $archiveTime
            $source = $_.OpenRead()
            $destination = $entry.Open()
            try {
                $source.CopyTo($destination)
            }
            finally {
                $destination.Dispose()
                $source.Dispose()
            }
        }
    }
    finally {
        $archive.Dispose()
    }
    $zipHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zipPath).Hash
    "$zipHash  $packageName.zip" | Set-Content -Encoding utf8 $zipHashPath
    Remove-Item -Recurse -Force -LiteralPath $stageRoot

    [pscustomobject]@{
        Package = $zipPath
        Bytes = (Get-Item -LiteralPath $zipPath).Length
        SHA256 = $zipHash
    }
}
finally {
    Pop-Location
    $currentEnvironment = [Environment]::GetEnvironmentVariables('Process')
    foreach ($name in $currentEnvironment.Keys) {
        if (-not $originalEnvironment.Contains($name)) {
            Remove-Item -LiteralPath "Env:$name"
        }
    }
    foreach ($name in $originalEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], 'Process')
    }
}
