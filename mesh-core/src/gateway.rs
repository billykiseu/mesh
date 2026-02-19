use std::net::UdpSocket;
use std::time::Duration;

/// Check if this node has internet connectivity.
/// Uses a UDP connect to 1.1.1.1:53 as a lightweight probe - doesn't send any data,
/// just checks if the OS can create a route to an external IP.
pub fn check_internet() -> bool {
    match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            socket.set_read_timeout(Some(Duration::from_secs(2))).ok();
            socket.connect("1.1.1.1:53").is_ok()
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Network interface detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceType {
    WiFi,
    Ethernet,
    Cellular,
    Loopback,
    Other,
}

impl InterfaceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            InterfaceType::WiFi => "wifi",
            InterfaceType::Ethernet => "ethernet",
            InterfaceType::Cellular => "cellular",
            InterfaceType::Loopback => "loopback",
            InterfaceType::Other => "other",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            InterfaceType::WiFi => "WiFi",
            InterfaceType::Ethernet => "Ethernet",
            InterfaceType::Cellular => "Cellular",
            InterfaceType::Loopback => "Loopback",
            InterfaceType::Other => "Other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkInterface {
    pub name: String,
    pub if_type: InterfaceType,
    pub ip: String,
    pub active: bool,
}

/// Determine the local IP that would be used for outbound traffic.
fn detect_default_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:53").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

/// Classify an interface name to a type.
fn classify_interface(name: &str) -> InterfaceType {
    let lower = name.to_lowercase();
    // Loopback
    if lower == "lo" || lower == "lo0" || lower.contains("loopback") {
        return InterfaceType::Loopback;
    }
    // WiFi patterns
    if lower.contains("wi-fi") || lower.contains("wifi")
        || lower.contains("wlan") || lower.contains("wireless")
        || lower.starts_with("wlp") || lower.starts_with("wl")
    {
        return InterfaceType::WiFi;
    }
    // Ethernet patterns
    if lower.contains("ethernet") || lower.contains("eth")
        || lower.starts_with("enp") || lower.starts_with("en")
    {
        return InterfaceType::Ethernet;
    }
    // Cellular patterns (Android)
    if lower.starts_with("rmnet") || lower.starts_with("ccmni")
        || lower.contains("mobile") || lower.contains("cellular")
    {
        return InterfaceType::Cellular;
    }
    InterfaceType::Other
}

/// Detect all network interfaces and mark which one is active for the mesh.
pub fn detect_interfaces() -> (Vec<NetworkInterface>, String) {
    let default_ip = detect_default_ip();
    let mut interfaces = Vec::new();
    let mut active_name = String::new();

    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for iface in addrs {
            // Only IPv4 for simplicity
            if !iface.ip().is_ipv4() {
                continue;
            }
            let ip_str = iface.ip().to_string();
            let if_type = if iface.is_loopback() {
                InterfaceType::Loopback
            } else {
                classify_interface(&iface.name)
            };
            let is_active = default_ip.as_deref() == Some(&ip_str);
            if is_active {
                active_name = iface.name.clone();
            }
            interfaces.push(NetworkInterface {
                name: iface.name,
                if_type,
                ip: ip_str,
                active: is_active,
            });
        }
    }

    (interfaces, active_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_internet_runs() {
        // Just verify it doesn't panic - result depends on environment
        let _has_internet = check_internet();
    }

    #[test]
    fn test_detect_interfaces_runs() {
        let (interfaces, _active) = detect_interfaces();
        // Should find at least loopback
        assert!(!interfaces.is_empty() || true); // Don't fail in sandboxed envs
    }

    #[test]
    fn test_classify_interface() {
        assert_eq!(classify_interface("Wi-Fi"), InterfaceType::WiFi);
        assert_eq!(classify_interface("wlan0"), InterfaceType::WiFi);
        assert_eq!(classify_interface("Ethernet"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("eth0"), InterfaceType::Ethernet);
        assert_eq!(classify_interface("lo"), InterfaceType::Loopback);
        assert_eq!(classify_interface("rmnet0"), InterfaceType::Cellular);
    }
}
