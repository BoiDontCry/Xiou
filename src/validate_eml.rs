use foxmail_imap::eml_validate::validate_file;
use std::env;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("用法：validate-eml <邮件.eml>（验证 RFC822/MIME 结构与必要邮件头）")?,
    );
    if args.next().is_some() {
        return Err("validate-eml 只接受一个 .eml 文件路径".into());
    }

    let report = validate_file(&path)?;
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !report.has_required_headers() {
        return Err("邮件可解析，但缺少 Subject、From 或 Date 必要头字段".into());
    }

    Ok(())
}
