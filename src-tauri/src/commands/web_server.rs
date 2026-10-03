use crate::{EffectiveWebBind, EffectiveWebPort, WebServerWarning};
use std::sync::atomic::Ordering;

/// Only accepts RFC 1918 private IPv4 and IPv6 ULA addresses.
/// Rejects loopback, unspecified, link-local, public, VPN/benchmarking ranges, etc.
fn is_lan_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            matches!(
                octets,
                [10, ..] |                                          // 10.0.0.0/8
                [172, 16..=31, ..] |                                // 172.16.0.0/12
                [192, 168, ..] // 192.168.0.0/16
            )
        }
        std::net::IpAddr::V6(v6) => {
            // fd00::/8 — Unique Local Address (ULA)
            v6.octets()[0] == 0xfd
        }
    }
}

/// Enumerate network interfaces and return the first LAN IP.
fn detect_lan_ip_from_interfaces(v6: bool) -> Option<String> {
    let ifaces = if_addrs::get_if_addrs().ok()?;
    for iface in &ifaces {
        let ip = iface.ip();
        let is_target_family = if v6 { ip.is_ipv6() } else { ip.is_ipv4() };
        if is_target_family && is_lan_ip(&ip) {
            log::debug!(
                "[web_server] found LAN IP via interface {}: {}",
                iface.name,
                ip
            );
            return Some(ip.to_string());
        }
    }
    None
}

fn detect_local_ip(v6: bool) -> Option<String> {
    // Primary: UDP socket probe (picks the default-route interface)
    let (bind, target) = if v6 {
        ("[::]:0", "[2001:4860:4860::8888]:80")
    } else {
        ("0.0.0.0:0", "8.8.8.8:80")
    };
    if let Some(ip) = (|| -> Option<std::net::IpAddr> {
        let socket = std::net::UdpSocket::bind(bind).ok()?;
        socket.connect(target).ok()?;
        Some(socket.local_addr().ok()?.ip())
    })() {
        if is_lan_ip(&ip) {
            return Some(ip.to_string());
        }
        log::debug!(
            "[web_server] UDP probe returned non-LAN IP {}, trying interface enumeration",
            ip
        );
    }

    // Fallback: enumerate network interfaces (handles VPN/proxy overriding default route)
    detect_lan_ip_from_interfaces(v6)
}

#[tauri::command]
pub fn get_local_ip(prefer_v6: bool) -> Result<Option<String>, String> {
    log::debug!("[web_server] get_local_ip(prefer_v6={})", prefer_v6);
    let result = detect_local_ip(prefer_v6);
    match &result {
        Some(ip) => log::debug!("[web_server] detected IP: {}", ip),
        None => log::warn!(
            "[web_server] IP detection failed ({})",
            if prefer_v6 { "IPv6 path" } else { "IPv4 path" }
        ),
    }
    Ok(result)
}

/// Get web server status (available via both IPC and WS)
#[tauri::command]
pub async fn get_web_server_status(
    effective_port: tauri::State<'_, EffectiveWebPort>,
    effective_bind: tauri::State<'_, EffectiveWebBind>,
    warning: tauri::State<'_, WebServerWarning>,
) -> Result<serde_json::Value, String> {
    log::debug!("[web_server] get_web_server_status");
    let port = effective_port.load(Ordering::Relaxed);
    let bind = effective_bind.0.read().await.clone();
    let warn = warning.0.read().await.clone();
    Ok(crate::web_server::build_status(port, &bind, &warn))
}

/// Get web server token from live memory (IPC-only, NOT exposed over WS)
#[tauri::command]
pub async fn get_web_server_token(
    live_token: tauri::State<'_, crate::SharedLiveToken>,
) -> Result<Option<String>, String> {
    log::debug!("[web_server] get_web_server_token (IPC-only)");
    let t = live_token.read().await;
    if t.is_empty() {
        Ok(None)
    } else {
        Ok(Some(t.clone()))
    }
}

/// Regenerate web server token — invalidates all existing sessions
#[tauri::command]
pub async fn regenerate_web_server_token(
    live_token: tauri::State<'_, crate::SharedLiveToken>,
    token_version: tauri::State<'_, crate::SharedTokenVersion>,
    ws_shutdown: tauri::State<'_, crate::WsShutdownSender>,
) -> Result<String, String> {
    use rand::Rng;
    let new_token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    *live_token.write().await = new_token.clone();
    token_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _ = ws_shutdown.send(());
    log::debug!("[web_server] token regenerated, all sessions invalidated");
    Ok(new_token)
}

/// Restart web server with new config.
/// Accepts config directly — backend does stop → start → save atomically.
#[tauri::command]
pub async fn restart_web_server(
    app: tauri::AppHandle,
    config: crate::web_server::WebServerConfig,
) -> Result<crate::web_server::RestartResult, String> {
    log::debug!(
        "[web_server] restart_web_server: enabled={}, port={}, bind={}, has_tunnel={}",
        config.enabled,
        config.port,
        config.bind,
        config.tunnel_url.is_some(),
    );
    // Desktop IPC path — adopt the handles already registered with the running app.
    let core = crate::web_server::state::CoreState::from_app(&app);
    crate::web_server::restart_with_config(&core, config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn is_lan_accepts_rfc1918() {
        assert!(is_lan_ip(&"10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_lan_ip(&"10.255.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_lan_ip(&"172.16.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_lan_ip(&"172.31.255.255".parse::<IpAddr>().unwrap()));
        assert!(is_lan_ip(&"192.168.1.1".parse::<IpAddr>().unwrap()));
        assert!(is_lan_ip(&"192.168.0.100".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_accepts_ipv6_ula() {
        assert!(is_lan_ip(&"fd12:3456:789a::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_rejects_loopback() {
        assert!(!is_lan_ip(&"127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(&"::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_rejects_unspecified() {
        assert!(!is_lan_ip(&"0.0.0.0".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(&"::".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_rejects_public() {
        assert!(!is_lan_ip(&"8.8.8.8".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(
            &"2001:4860:4860::8888".parse::<IpAddr>().unwrap()
        ));
    }

    #[test]
    fn is_lan_rejects_benchmarking() {
        // 198.18.0.0/15 — IANA benchmarking, common VPN/proxy interfaces
        assert!(!is_lan_ip(&"198.18.0.1".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(&"198.19.255.255".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_rejects_link_local() {
        assert!(!is_lan_ip(&"169.254.1.1".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(&"fe80::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn is_lan_rejects_172_outside_range() {
        // 172.15.x and 172.32.x are NOT private
        assert!(!is_lan_ip(&"172.15.0.1".parse::<IpAddr>().unwrap()));
        assert!(!is_lan_ip(&"172.32.0.1".parse::<IpAddr>().unwrap()));
    }
}
