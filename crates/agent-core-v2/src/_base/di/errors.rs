use super::{graph::Graph, instantiation::ErasedServiceIdentifier};

#[derive(Debug, thiserror::Error)]
pub enum DiError {
    #[error("unknown service '{0}'")]
    UnknownService(ErasedServiceIdentifier),

    #[error("service '{0}' has an incompatible registered type")]
    TypeMismatch(ErasedServiceIdentifier),

    #[error("Cyclic DI dependency detected: {path}", path = .0.join(" → "))]
    CyclicDependency(Vec<String>),

    #[error("instantiation service has been disposed")]
    Disposed,

    #[error("service factory failed: {0}")]
    Factory(String),
}

impl DiError {
    pub fn cyclic_from_graph<T, H>(graph: &Graph<T, H>) -> Self
    where
        H: Fn(&T) -> String,
    {
        match graph.find_cycle_slow() {
            Some(cycle) => Self::CyclicDependency(cycle.split(" -> ").map(str::to_owned).collect()),
            None => Self::Factory(format!(
                "cyclic dependency between services: UNABLE to detect cycle, dumping graph:\n{graph}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_errors_preserve_unicode_and_graph_path_forms() {
        let path = DiError::CyclicDependency(vec!["a".into(), "b".into()]);
        assert_eq!(path.to_string(), "Cyclic DI dependency detected: a → b");

        let mut graph = Graph::new(Clone::clone);
        graph.insert_edge("a".to_owned(), "b".to_owned());
        graph.insert_edge("b".to_owned(), "a".to_owned());
        let error = DiError::cyclic_from_graph(&graph);
        assert!(matches!(error, DiError::CyclicDependency(_)));
    }
}
