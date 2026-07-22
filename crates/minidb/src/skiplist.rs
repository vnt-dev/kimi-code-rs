use std::{cmp::Ordering, sync::Arc};

const MAX_LEVEL: usize = 32;

pub type Comparator<T> = Arc<dyn Fn(&T, &T) -> Ordering + Send + Sync>;

pub fn compare_number(left: &f64, right: &f64) -> Ordering {
    left.partial_cmp(right).unwrap_or(Ordering::Equal)
}

pub fn compare_string(left: &String, right: &String) -> Ordering {
    left.cmp(right)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEntry<K, V> {
    pub key: K,
    pub value: V,
}

#[derive(Debug, Clone)]
pub struct RangeOptions<K> {
    pub gte: Option<K>,
    pub gt: Option<K>,
    pub lte: Option<K>,
    pub lt: Option<K>,
    pub offset: usize,
    pub count: Option<usize>,
    pub reverse: bool,
}

impl<K> Default for RangeOptions<K> {
    fn default() -> Self {
        Self {
            gte: None,
            gt: None,
            lte: None,
            lt: None,
            offset: 0,
            count: None,
            reverse: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Level {
    forward: Option<usize>,
    span: usize,
}

struct Node<K, V> {
    key: Option<K>,
    value: Option<V>,
    backward: Option<usize>,
    levels: Vec<Level>,
}

impl<K, V> Node<K, V> {
    fn header() -> Self {
        Self {
            key: None,
            value: None,
            backward: None,
            levels: vec![
                Level {
                    forward: None,
                    span: 0
                };
                MAX_LEVEL
            ],
        }
    }

    fn new(key: K, value: V, level: usize) -> Self {
        Self {
            key: Some(key),
            value: Some(value),
            backward: None,
            levels: vec![
                Level {
                    forward: None,
                    span: 0
                };
                level
            ],
        }
    }
}

pub struct SkipList<K, V> {
    compare_key: Comparator<K>,
    compare_value: Comparator<V>,
    nodes: Vec<Node<K, V>>,
    tail: Option<usize>,
    len: usize,
    level: usize,
    random_state: u64,
}

impl<K, V> SkipList<K, V> {
    pub fn with_comparators(
        compare_key: impl Fn(&K, &K) -> Ordering + Send + Sync + 'static,
        compare_value: impl Fn(&V, &V) -> Ordering + Send + Sync + 'static,
    ) -> Self {
        Self {
            compare_key: Arc::new(compare_key),
            compare_value: Arc::new(compare_value),
            nodes: vec![Node::header()],
            tail: None,
            len: 0,
            level: 1,
            random_state: 0x6a09_e667_f3bc_c909,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn random_level(&mut self) -> usize {
        let mut level = 1;
        loop {
            self.random_state ^= self.random_state << 13;
            self.random_state ^= self.random_state >> 7;
            self.random_state ^= self.random_state << 17;
            if self.random_state & 3 != 0 || level == MAX_LEVEL {
                return level;
            }
            level += 1;
        }
    }

    fn node_less(&self, node: usize, key: &K, value: &V) -> bool {
        let node = &self.nodes[node];
        match (self.compare_key)(node.key.as_ref().expect("data node"), key) {
            Ordering::Less => true,
            Ordering::Equal => {
                (self.compare_value)(node.value.as_ref().expect("data node"), value)
                    == Ordering::Less
            }
            Ordering::Greater => false,
        }
    }

    // Original: packages/minidb/src/skiplist.ts, SkipList.insert().
    pub fn insert(&mut self, key: K, value: V) {
        let mut update = [0_usize; MAX_LEVEL];
        let mut rank = [0_usize; MAX_LEVEL];
        let mut current = 0;
        for index in (0..self.level).rev() {
            rank[index] = if index == self.level - 1 {
                0
            } else {
                rank[index + 1]
            };
            while let Some(forward) = self.nodes[current].levels[index].forward {
                if !self.node_less(forward, &key, &value) {
                    break;
                }
                rank[index] += self.nodes[current].levels[index].span;
                current = forward;
            }
            update[index] = current;
        }

        let node_level = self.random_level();
        if node_level > self.level {
            for index in self.level..node_level {
                rank[index] = 0;
                update[index] = 0;
                self.nodes[0].levels[index].span = self.len;
            }
            self.level = node_level;
        }

        let node_index = self.nodes.len();
        self.nodes.push(Node::new(key, value, node_level));
        for index in 0..node_level {
            let previous = update[index];
            let previous_level = self.nodes[previous].levels[index];
            self.nodes[node_index].levels[index].forward = previous_level.forward;
            self.nodes[node_index].levels[index].span =
                previous_level.span.saturating_sub(rank[0] - rank[index]);
            self.nodes[previous].levels[index].forward = Some(node_index);
            self.nodes[previous].levels[index].span = rank[0] - rank[index] + 1;
        }
        for (index, previous) in update.iter().enumerate().take(self.level).skip(node_level) {
            self.nodes[*previous].levels[index].span += 1;
        }

        let previous = update[0];
        self.nodes[node_index].backward = (previous != 0).then_some(previous);
        if let Some(forward) = self.nodes[node_index].levels[0].forward {
            self.nodes[forward].backward = Some(node_index);
        } else {
            self.tail = Some(node_index);
        }
        self.len += 1;
    }

    // Original: packages/minidb/src/skiplist.ts, SkipList.delete().
    pub fn delete(&mut self, key: &K, value: &V) -> bool {
        let mut update = [0_usize; MAX_LEVEL];
        let mut current = 0;
        for index in (0..self.level).rev() {
            while let Some(forward) = self.nodes[current].levels[index].forward {
                if !self.node_less(forward, key, value) {
                    break;
                }
                current = forward;
            }
            update[index] = current;
        }
        let Some(target) = self.nodes[current].levels[0].forward else {
            return false;
        };
        if (self.compare_key)(self.nodes[target].key.as_ref().expect("data node"), key)
            != Ordering::Equal
            || (self.compare_value)(self.nodes[target].value.as_ref().expect("data node"), value)
                != Ordering::Equal
        {
            return false;
        }

        for (index, previous) in update.iter().enumerate().take(self.level) {
            if self.nodes[*previous].levels[index].forward == Some(target) {
                let target_span = self.nodes[target]
                    .levels
                    .get(index)
                    .map_or(0, |level| level.span);
                self.nodes[*previous].levels[index].span = self.nodes[*previous].levels[index]
                    .span
                    .saturating_add(target_span)
                    .saturating_sub(1);
                self.nodes[*previous].levels[index].forward = self.nodes[target]
                    .levels
                    .get(index)
                    .and_then(|level| level.forward);
            } else {
                self.nodes[*previous].levels[index].span =
                    self.nodes[*previous].levels[index].span.saturating_sub(1);
            }
        }
        let forward = self.nodes[target].levels[0].forward;
        if let Some(forward) = forward {
            self.nodes[forward].backward = self.nodes[target].backward;
        } else {
            self.tail = self.nodes[target].backward;
        }
        while self.level > 1 && self.nodes[0].levels[self.level - 1].forward.is_none() {
            self.level -= 1;
        }
        self.len -= 1;
        true
    }

    fn lower_bound_index(&self, bound: &K, strict: bool) -> Option<usize> {
        let mut current = 0;
        for index in (0..self.level).rev() {
            while let Some(forward) = self.nodes[current].levels[index].forward {
                let ordering =
                    (self.compare_key)(self.nodes[forward].key.as_ref().expect("data node"), bound);
                if if strict {
                    ordering.is_le()
                } else {
                    ordering.is_lt()
                } {
                    current = forward;
                } else {
                    break;
                }
            }
        }
        self.nodes[current].levels[0].forward
    }
}

impl<K: Clone, V: Clone> SkipList<K, V> {
    fn entry(&self, index: usize) -> RangeEntry<K, V> {
        RangeEntry {
            key: self.nodes[index].key.as_ref().expect("data node").clone(),
            value: self.nodes[index].value.as_ref().expect("data node").clone(),
        }
    }

    pub fn lower_bound(&self, bound: &K, strict: bool) -> Option<RangeEntry<K, V>> {
        self.lower_bound_index(bound, strict)
            .map(|index| self.entry(index))
    }

    // Original: packages/minidb/src/skiplist.ts, SkipList.getRank().
    pub fn rank(&self, key: &K, value: &V) -> Option<usize> {
        let mut current = 0;
        let mut rank = 0;
        for index in (0..self.level).rev() {
            while let Some(forward) = self.nodes[current].levels[index].forward {
                let same =
                    (self.compare_key)(self.nodes[forward].key.as_ref().expect("data node"), key)
                        == Ordering::Equal
                        && (self.compare_value)(
                            self.nodes[forward].value.as_ref().expect("data node"),
                            value,
                        ) == Ordering::Equal;
                if !self.node_less(forward, key, value) && !same {
                    break;
                }
                rank += self.nodes[current].levels[index].span;
                current = forward;
            }
        }
        (current != 0
            && (self.compare_key)(self.nodes[current].key.as_ref().expect("data node"), key)
                == Ordering::Equal
            && (self.compare_value)(
                self.nodes[current].value.as_ref().expect("data node"),
                value,
            ) == Ordering::Equal)
            .then_some(rank - 1)
    }

    // Original: packages/minidb/src/skiplist.ts, SkipList.getByRank().
    pub fn get_by_rank(&self, rank: usize) -> Option<RangeEntry<K, V>> {
        if rank >= self.len {
            return None;
        }
        let target = rank + 1;
        let mut current = 0;
        let mut traversed = 0;
        for index in (0..self.level).rev() {
            while let Some(forward) = self.nodes[current].levels[index].forward {
                if traversed + self.nodes[current].levels[index].span > target {
                    break;
                }
                traversed += self.nodes[current].levels[index].span;
                current = forward;
            }
            if traversed == target {
                return Some(self.entry(current));
            }
        }
        None
    }

    // Original: packages/minidb/src/skiplist.ts, SkipList.range()/iterate().
    pub fn range(&self, options: &RangeOptions<K>) -> Vec<RangeEntry<K, V>> {
        let mut output = Vec::new();
        let mut offset = options.offset;
        let mut remaining = options.count.unwrap_or(usize::MAX);
        let mut current = if options.reverse {
            if let Some(bound) = options.lte.as_ref() {
                self.lower_bound_index(bound, true)
                    .and_then(|index| self.nodes[index].backward)
                    .or_else(|| {
                        self.lower_bound_index(bound, true)
                            .is_none()
                            .then_some(self.tail)
                            .flatten()
                    })
            } else if let Some(bound) = options.lt.as_ref() {
                self.lower_bound_index(bound, false)
                    .and_then(|index| self.nodes[index].backward)
                    .or_else(|| {
                        self.lower_bound_index(bound, false)
                            .is_none()
                            .then_some(self.tail)
                            .flatten()
                    })
            } else {
                self.tail
            }
        } else if let Some(bound) = options.gte.as_ref() {
            self.lower_bound_index(bound, false)
        } else if let Some(bound) = options.gt.as_ref() {
            self.lower_bound_index(bound, true)
        } else {
            self.nodes[0].levels[0].forward
        };

        while let Some(index) = current {
            let key = self.nodes[index].key.as_ref().expect("data node");
            let in_bounds = if options.reverse {
                options
                    .gte
                    .as_ref()
                    .is_none_or(|bound| (self.compare_key)(key, bound).is_ge())
                    && options
                        .gt
                        .as_ref()
                        .is_none_or(|bound| (self.compare_key)(key, bound).is_gt())
            } else {
                options
                    .lte
                    .as_ref()
                    .is_none_or(|bound| (self.compare_key)(key, bound).is_le())
                    && options
                        .lt
                        .as_ref()
                        .is_none_or(|bound| (self.compare_key)(key, bound).is_lt())
            };
            if !in_bounds {
                break;
            }
            if offset > 0 {
                offset -= 1;
            } else if remaining > 0 {
                output.push(self.entry(index));
                remaining -= 1;
            } else {
                break;
            }
            current = if options.reverse {
                self.nodes[index].backward
            } else {
                self.nodes[index].levels[0].forward
            };
        }
        output
    }

    pub fn iter(&self, options: &RangeOptions<K>) -> impl Iterator<Item = RangeEntry<K, V>> {
        self.range(options).into_iter()
    }

    pub fn to_vec(&self) -> Vec<RangeEntry<K, V>> {
        self.range(&RangeOptions::default())
    }
}

impl<K: Ord + 'static, V: Ord + 'static> Default for SkipList<K, V> {
    fn default() -> Self {
        Self::with_comparators(K::cmp, V::cmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order_ranks_ranges_and_deletion() {
        let mut list = SkipList::<i64, String>::default();
        for (key, value) in [(3, "c"), (1, "a"), (2, "b"), (2, "a")] {
            list.insert(key, value.to_owned());
        }
        assert_eq!(
            list.to_vec()
                .into_iter()
                .map(|entry| (entry.key, entry.value))
                .collect::<Vec<_>>(),
            vec![
                (1, "a".into()),
                (2, "a".into()),
                (2, "b".into()),
                (3, "c".into())
            ]
        );
        assert_eq!(list.rank(&2, &"b".to_owned()), Some(2));
        assert_eq!(
            list.get_by_rank(2).map(|entry| entry.value),
            Some("b".into())
        );
        assert!(list.delete(&2, &"a".to_owned()));
        assert!(!list.delete(&2, &"a".to_owned()));

        let range = list.range(&RangeOptions {
            gte: Some(2),
            lte: Some(3),
            reverse: true,
            ..RangeOptions::default()
        });
        assert_eq!(
            range.into_iter().map(|entry| entry.key).collect::<Vec<_>>(),
            vec![3, 2]
        );
    }
}
