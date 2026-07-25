use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

/// Common projection of a live WebSocket connection.
pub trait ConnectionLike: Send + Sync {
    fn id(&self) -> &str;
    fn connected_at(&self) -> &str;
    fn remote_address(&self) -> Option<&str>;
    fn user_agent(&self) -> Option<&str>;
    fn has_client_hello(&self) -> bool;
    fn subscription_session_ids(&self) -> Vec<String>;
    fn close(&self, code: u16, reason: Option<&str>);
}

/// Server-local live WebSocket registry.
#[derive(Default)]
pub struct ConnectionRegistry {
    connections: RwLock<IndexMap<String, Arc<dyn ConnectionLike>>>,
}

impl std::fmt::Debug for ConnectionRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionRegistry")
            .field("size", &self.size())
            .finish()
    }
}

impl ConnectionRegistry {
    // Original: connectionRegistry.ts, ConnectionRegistry.add().
    pub fn add(&self, connection: Arc<dyn ConnectionLike>) {
        self.connections
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .insert(connection.id().to_owned(), connection);
    }

    pub fn remove(&self, connection_id: &str) {
        self.connections
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .shift_remove(connection_id);
    }

    pub fn get(&self, connection_id: &str) -> Option<Arc<dyn ConnectionLike>> {
        self.connections
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(connection_id)
            .cloned()
    }

    pub fn values(&self) -> Vec<Arc<dyn ConnectionLike>> {
        self.connections
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect()
    }

    // Original: ConnectionRegistry.closeAll().
    // Clear before invoking callbacks so reentrant reads observe an empty
    // registry and a failing close cannot prevent later connections closing.
    pub fn close_all(&self, reason: Option<&str>) {
        let snapshot = {
            let mut connections = self
                .connections
                .write()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *connections)
        };
        for connection in snapshot.values() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                connection.close(1001, reason);
            }));
        }
    }

    pub fn size(&self) -> usize {
        self.connections
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct MockConnection {
        id: String,
        closes: AtomicUsize,
        panic_on_close: bool,
    }

    impl ConnectionLike for MockConnection {
        fn id(&self) -> &str {
            &self.id
        }
        fn connected_at(&self) -> &str {
            "2026-01-01T00:00:00Z"
        }
        fn remote_address(&self) -> Option<&str> {
            None
        }
        fn user_agent(&self) -> Option<&str> {
            None
        }
        fn has_client_hello(&self) -> bool {
            false
        }
        fn subscription_session_ids(&self) -> Vec<String> {
            Vec::new()
        }
        fn close(&self, code: u16, _reason: Option<&str>) {
            assert_eq!(code, 1001);
            self.closes.fetch_add(1, Ordering::SeqCst);
            assert!(!self.panic_on_close, "simulated close failure");
        }
    }

    fn connection(id: &str, panic_on_close: bool) -> Arc<MockConnection> {
        Arc::new(MockConnection {
            id: id.into(),
            closes: AtomicUsize::new(0),
            panic_on_close,
        })
    }

    #[test]
    fn adds_replaces_gets_and_removes_connections() {
        let registry = ConnectionRegistry::default();
        let first = connection("conn_1", false);
        registry.add(Arc::clone(&first) as Arc<dyn ConnectionLike>);
        assert_eq!(registry.size(), 1);
        assert_eq!(registry.get("conn_1").unwrap().id(), "conn_1");

        let replacement = connection("conn_1", false);
        registry.add(Arc::clone(&replacement) as Arc<dyn ConnectionLike>);
        assert_eq!(registry.size(), 1);
        assert!(Arc::ptr_eq(
            &registry.get("conn_1").unwrap(),
            &(replacement as Arc<dyn ConnectionLike>)
        ));
        registry.remove("conn_1");
        registry.remove("conn_1");
        assert!(registry.is_empty());
    }

    #[test]
    fn close_all_is_best_effort_and_clears_first() {
        let registry = ConnectionRegistry::default();
        let failing = connection("bad", true);
        let succeeding = connection("good", false);
        registry.add(Arc::clone(&failing) as Arc<dyn ConnectionLike>);
        registry.add(Arc::clone(&succeeding) as Arc<dyn ConnectionLike>);
        registry.close_all(Some("shutdown"));
        assert!(registry.is_empty());
        assert_eq!(failing.closes.load(Ordering::SeqCst), 1);
        assert_eq!(succeeding.closes.load(Ordering::SeqCst), 1);
    }
}
