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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_internet_runs() {
        // Just verify it doesn't panic - result depends on environment
        let _has_internet = check_internet();
    }
}
