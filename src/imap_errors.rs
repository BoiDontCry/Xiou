use std::fmt::Display;
use std::io;

const PASSWORD_POLICY_MARKER: &str = "Password fails to satisfy the password policy";

pub fn login_error(error: impl Display) -> io::Error {
    let message = error.to_string();
    if message.contains(PASSWORD_POLICY_MARKER) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "服务器拒绝登录：账号密码不符合当前安全策略。请先登录 Web 邮箱修改密码；若管理员启用了客户端专用密码，请使用授权码重新执行 --reset-password。服务器原始信息：{message}"
            ),
        );
    }

    let lower = message.to_lowercase();
    let hint = if lower.contains("invalid credentials")
        || lower.contains("authentication failed")
        || lower.contains("login failed")
        || lower.contains("auth")
    {
        "请检查邮箱地址与密码/授权码是否正确（Web 端可登录即说明密码对，客户端可能需要专用授权码）。"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "连接超时，请检查网络和端口是否正确（IMAP 隐式 TLS 通常为 993）。"
    } else {
        "请核对收信服务器地址与端口；密码修改过的话请在账号设置里更新。"
    };
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("IMAP 登录失败：{message}。{hint}"),
    )
}

/// 给同步链路的连接/IO 错误加"下一步怎么做"引导；登录错误已带引导直接放行。
pub fn connect_error(error: impl Display) -> io::Error {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let hint = if lower.contains("timed out") || lower.contains("timeout") {
        "连接超时，请检查网络和端口是否正确（IMAP 隐式 TLS 通常为 993）。"
    } else if lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake")
    {
        "TLS 握手失败：请确认端口与加密方式匹配（993 隐式 TLS；143/STARTTLS 本工具暂不支持）。"
    } else if lower.contains("refused")
        || lower.contains("unreachable")
        || lower.contains("failed to lookup")
    {
        "无法连通收信服务器：请核对服务器地址与端口，并确认本机网络可用。"
    } else {
        "连接收信服务器失败，请稍后重试或核对账号设置。"
    };
    io::Error::other(format!("{message}。{hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_password_policy_failure() {
        let error = login_error(
            "Password fails to satisfy the password policy, login denied, please retry after modification.",
        );
        let text = error.to_string();
        assert!(text.contains("Web 邮箱修改密码"));
        assert!(text.contains("客户端专用密码"));
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn preserves_other_login_error_details() {
        let error = login_error("authentication failed");
        assert!(error.to_string().contains("authentication failed"));
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn login_failure_gets_auth_hint() {
        let error = login_error("[AUTHENTICATIONFAILED] Invalid credentials");
        let text = error.to_string();
        assert!(text.contains("授权码"), "{text}");
        assert!(text.contains("Invalid credentials"), "{text}");
    }

    #[test]
    fn connect_failures_get_actionable_hints() {
        let timeout = connect_error("connection timed out");
        assert!(timeout.to_string().contains("993"), "{timeout}");

        let tls = connect_error("tls handshake failure: certificate verify failed");
        assert!(tls.to_string().contains("TLS 握手失败"), "{tls}");

        let refused = connect_error("Connection refused (os error 111)");
        assert!(refused.to_string().contains("核对服务器地址"), "{refused}");
    }
}
