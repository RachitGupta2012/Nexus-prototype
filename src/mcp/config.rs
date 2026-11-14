//! Validation for persisted MCP server settings.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use crate::config::{McpServerConfig, McpTransport};

pub const MAX_NAME_BYTES: usize = 128;
pub const MAX_COMMAND_BYTES: usize = 4_096;
pub const MAX_ARG_BYTES: usize = 8_192;
pub const MAX_ARGS: usize = 128;
pub const MAX_URL_BYTES: usize = 2_048;
pub const MAX_ENV_NAMES: usize = 32;

/// Validates a server entry without performing I/O. Errors are safe to show.
pub fn validate(server: &McpServerConfig) -> Result<(), &'static str> {
    let name = server.name.trim();
    if name.is_empty() {
        return Err("Give the server a non-empty name.");
    }
    if name.len() > MAX_NAME_BYTES {
        return Err("The server name is too long.");
    }
    match server.transport {
        McpTransport::Stdio => validate_stdio(server),
        McpTransport::StreamableHttp => validate_http(server),
    }
}

fn validate_stdio(server: &McpServerConfig) -> Result<(), &'static str> {
    let command = server.command.trim();
    if command.is_empty() {
        return Err("Set the executable to launch for this STDIO server.");
    }
    if command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return Err("The executable path is invalid or too long.");
    }
    if server.args.len() > MAX_ARGS {
        return Err("Too many arguments.");
    }
    if server
        .args
        .iter()
        .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
    {
        return Err("An argument is invalid or too long.");
    }
    if let Some(cwd) = &server.cwd {
        let cwd = cwd.trim();
        if cwd.is_empty()
            || cwd.len() > MAX_COMMAND_BYTES
            || cwd.contains('\0')
            || cwd.contains('\n')
        {
            return Err("The working directory is invalid.");
        }
        if !std::path::Path::new(cwd).is_absolute() {
            return Err("The working directory must be an absolute path.");
        }
    }
    if server.env_allowlist.len() > MAX_ENV_NAMES
        || server.env_allowlist.iter().any(|name| {
            name.is_empty()
                || name.len() > 128
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    {
        return Err("Environment variable names must use letters, digits, and underscores.");
    }
    if !server.url.trim().is_empty() {
        return Err("A STDIO server cannot also have a URL.");
    }
    Ok(())
}

fn validate_http(server: &McpServerConfig) -> Result<(), &'static str> {
    let raw = server.url.trim();
    if raw.is_empty() {
        return Err("Set the Streamable HTTP endpoint URL.");
    }
    if raw.len() > MAX_URL_BYTES {
        return Err("The endpoint URL is too long.");
    }
    let url = Url::parse(raw).map_err(|_| "The endpoint URL is not valid.")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Credentials must not be embedded in the endpoint URL.");
    }
    let host = url.host_str().ok_or("The endpoint URL needs a host.")?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(host) => {}
        _ => return Err("Use HTTPS, or HTTP only for a loopback endpoint."),
    }
    if url.fragment().is_some() {
        return Err("The endpoint URL must not contain a fragment.");
    }
    if !server.command.trim().is_empty() || !server.args.is_empty() {
        return Err("A Streamable HTTP server cannot also have a command or arguments.");
    }
    if server
        .cwd
        .as_ref()
        .is_some_and(|cwd| !cwd.trim().is_empty())
    {
        return Err("A Streamable HTTP server cannot have a working directory.");
    }
    Ok(())
}

fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.parse::<IpAddr>().is_ok_and(|addr| match addr {
        IpAddr::V4(addr) => addr.is_loopback() || addr == Ipv4Addr::UNSPECIFIED,
        IpAddr::V6(addr) => addr.is_loopback() || addr == Ipv6Addr::UNSPECIFIED,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(transport: McpTransport) -> McpServerConfig {
        McpServerConfig {
            id: 1,
            name: "Test".to_owned(),
            transport,
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env_allowlist: Vec::new(),
            url: String::new(),
            bearer_auth: false,
            enabled_tools: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn stdio_requires_a_command_and_rejects_shell_like_extras() {
        let mut settings = server(McpTransport::Stdio);
        assert!(validate(&settings).is_err());
        settings.command = "node".to_owned();
        settings.args = vec!["server.js".to_owned()];
        assert!(validate(&settings).is_ok());
        settings.url = "https://example.com".to_owned();
        assert!(validate(&settings).is_err());
        settings.url.clear();
        settings.cwd = Some("relative".to_owned());
        assert!(validate(&settings).is_err());
        settings.cwd = Some("/tmp".to_owned());
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn http_requires_https_except_loopback_and_rejects_embedded_credentials() {
        let mut settings = server(McpTransport::StreamableHttp);
        settings.url = "http://example.com/mcp".to_owned();
        assert!(validate(&settings).is_err());
        settings.url = "https://example.com/mcp".to_owned();
        assert!(validate(&settings).is_ok());
        settings.url = "http://127.0.0.1:8080/mcp".to_owned();
        assert!(validate(&settings).is_ok());
        settings.url = "http://[::1]:8080/mcp".to_owned();
        assert!(validate(&settings).is_ok());
        settings.url = "https://user:secret@example.com/mcp".to_owned();
        assert!(validate(&settings).is_err());
        settings.url = "ftp://example.com/mcp".to_owned();
        assert!(validate(&settings).is_err());
        settings.url = "https://example.com/mcp".to_owned();
        settings.command = "node".to_owned();
        assert!(validate(&settings).is_err());
    }
}
