//! Free port selection. Port of `server/net.js`.
//!
//! The table listens on loopback only; iroh owns the real network. The single
//! networking question here is which port to take.

/// Tries to actually bind the port, the same way the table will.
pub async fn is_port_free(port: u16) -> bool {
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .is_ok()
}

/// Scans `preferred..preferred + count`, falls back to 0 (OS picks).
pub async fn find_free_port(preferred: u16, count: u16) -> u16 {
    for offset in 0..count {
        let Some(candidate) = preferred.checked_add(offset) else {
            break;
        };
        if is_port_free(candidate).await {
            return candidate;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_a_port_in_range() {
        let port = find_free_port(32100, 64).await;
        assert!(port == 0 || (32100..32164).contains(&port));
    }

    #[tokio::test]
    async fn busy_port_is_skipped() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let busy = listener.local_addr().unwrap().port();
        assert!(!is_port_free(busy).await);
    }
}
