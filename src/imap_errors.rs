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

    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("IMAP 登录失败：{message}"),
    )
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
}
