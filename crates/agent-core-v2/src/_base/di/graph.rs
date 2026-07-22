use indexmap::{IndexMap, IndexSet};

#[derive(Debug)]
pub struct Node<T> {
    pub key: String,
    pub data: T,
    incoming: IndexSet<String>,
    outgoing: IndexSet<String>,
}

impl<T> Node<T> {
    pub fn incoming(&self) -> impl Iterator<Item = &str> {
        self.incoming.iter().map(String::as_str)
    }

    pub fn outgoing(&self) -> impl Iterator<Item = &str> {
        self.outgoing.iter().map(String::as_str)
    }
}

pub struct Graph<T, H> {
    nodes: IndexMap<String, Node<T>>,
    hash: H,
}

impl<T, H> Graph<T, H>
where
    H: Fn(&T) -> String,
{
    pub fn new(hash: H) -> Self {
        Self {
            nodes: IndexMap::new(),
            hash,
        }
    }

    // Original: packages/agent-core-v2/src/_base/di/graph.ts, Graph.roots().
    pub fn roots(&self) -> Vec<&Node<T>> {
        self.nodes
            .values()
            .filter(|node| node.outgoing.is_empty())
            .collect()
    }

    pub fn insert_edge(&mut self, from: T, to: T) {
        let from_key = self.lookup_or_insert_node(from).key.clone();
        let to_key = self.lookup_or_insert_node(to).key.clone();
        self.nodes
            .get_mut(&from_key)
            .expect("inserted graph node must exist")
            .outgoing
            .insert(to_key.clone());
        self.nodes
            .get_mut(&to_key)
            .expect("inserted graph node must exist")
            .incoming
            .insert(from_key);
    }

    pub fn remove_node(&mut self, data: &T) -> Option<T> {
        let key = (self.hash)(data);
        let removed = self.nodes.shift_remove(&key)?.data;
        for node in self.nodes.values_mut() {
            node.outgoing.shift_remove(&key);
            node.incoming.shift_remove(&key);
        }
        Some(removed)
    }

    pub fn lookup_or_insert_node(&mut self, data: T) -> &mut Node<T> {
        let key = (self.hash)(&data);
        self.nodes.entry(key.clone()).or_insert_with(|| Node {
            key,
            data,
            incoming: IndexSet::new(),
            outgoing: IndexSet::new(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn find_cycle_slow(&self) -> Option<String> {
        for (id, node) in &self.nodes {
            let mut seen = IndexSet::from([id.clone()]);
            if let Some(cycle) = self.find_cycle(node, &mut seen) {
                return Some(cycle);
            }
        }
        None
    }

    fn find_cycle(&self, node: &Node<T>, seen: &mut IndexSet<String>) -> Option<String> {
        for id in &node.outgoing {
            if seen.contains(id) {
                return Some(
                    seen.iter()
                        .chain(std::iter::once(id))
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" -> "),
                );
            }
            seen.insert(id.clone());
            if let Some(outgoing) = self.nodes.get(id)
                && let Some(cycle) = self.find_cycle(outgoing, seen)
            {
                return Some(cycle);
            }
            seen.shift_remove(id);
        }
        None
    }
}

impl<T, H> std::fmt::Display for Graph<T, H>
where
    H: Fn(&T) -> String,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut entries = Vec::with_capacity(self.nodes.len());
        for (key, node) in &self.nodes {
            entries.push(format!(
                "{key}\n\t(-> incoming)[{}]\n\t(outgoing ->)[{}]\n",
                node.incoming.iter().cloned().collect::<Vec<_>>().join(", "),
                node.outgoing.iter().cloned().collect::<Vec<_>>().join(",")
            ));
        }
        formatter.write_str(&entries.join("\n"))
    }
}

pub fn identity_hash<T>(value: &T) -> String
where
    T: ToString,
{
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_edges_and_removal_preserve_insertion_order() {
        let mut graph = Graph::new(Clone::clone);
        graph.insert_edge("service".to_owned(), "dependency-a".to_owned());
        graph.insert_edge("service".to_owned(), "dependency-b".to_owned());
        graph.insert_edge("dependency-a".to_owned(), "leaf".to_owned());

        assert_eq!(
            graph
                .roots()
                .into_iter()
                .map(|node| node.data.as_str())
                .collect::<Vec<_>>(),
            vec!["dependency-b", "leaf"]
        );
        assert_eq!(
            graph.remove_node(&"dependency-a".to_owned()),
            Some("dependency-a".to_owned())
        );
        assert_eq!(graph.roots()[0].data, "dependency-b");
        assert!(graph.find_cycle_slow().is_none());
    }

    #[test]
    fn cycle_path_matches_source_depth_first_shape() {
        let mut graph = Graph::new(Clone::clone);
        graph.insert_edge("a".to_owned(), "b".to_owned());
        graph.insert_edge("b".to_owned(), "c".to_owned());
        graph.insert_edge("c".to_owned(), "b".to_owned());
        assert_eq!(graph.find_cycle_slow().as_deref(), Some("a -> b -> c -> b"));
    }

    #[test]
    fn display_matches_source_punctuation() {
        let mut graph = Graph::new(Clone::clone);
        graph.insert_edge("a".to_owned(), "b".to_owned());
        assert_eq!(
            graph.to_string(),
            "a\n\t(-> incoming)[]\n\t(outgoing ->)[b]\n\nb\n\t(-> incoming)[a]\n\t(outgoing ->)[]\n"
        );
    }
}
