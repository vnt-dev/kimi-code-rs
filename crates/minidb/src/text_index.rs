use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::OnceLock,
};

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::{
    query::get_path,
    text_postings::{PostingEntry, PostingsError, PostingsFile},
};

const MAX_TERM_BYTES: usize = u16::MAX as usize;

#[derive(Debug, Clone, Default)]
pub struct TextIndexOptions {
    pub fields: Option<Vec<String>>,
    pub postings_path: Option<PathBuf>,
    pub cache_terms: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOperator {
    #[default]
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub operator: SearchOperator,
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            operator: SearchOperator::And,
            limit: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub key: String,
    pub score: f64,
}

#[derive(Debug, Error)]
pub enum TextIndexError {
    #[error(transparent)]
    Postings(#[from] PostingsError),
}

pub struct TextIndex {
    fields: Option<Vec<String>>,
    path: Option<PathBuf>,
    cache_terms: usize,
    pub postings: HashMap<String, PostingEntry>,
    document_lengths: HashMap<u32, usize>,
    keys: Vec<Option<String>>,
    key_to_id: HashMap<String, u32>,
    delta: HashMap<String, BTreeMap<u32, u32>>,
    delta_count: usize,
    removed: HashSet<u32>,
    memory_base: Option<HashMap<String, BTreeMap<u32, u32>>>,
    postings_file: Option<PostingsFile>,
    cache: HashMap<String, Vec<(u32, u32)>>,
    cache_order: Vec<String>,
    document_count: usize,
}

impl TextIndex {
    pub fn new(options: TextIndexOptions) -> Self {
        let memory_base = options.postings_path.is_none().then(HashMap::new);
        Self {
            fields: options.fields,
            path: options.postings_path,
            cache_terms: options.cache_terms.unwrap_or(1_024),
            postings: HashMap::new(),
            document_lengths: HashMap::new(),
            keys: Vec::new(),
            key_to_id: HashMap::new(),
            delta: HashMap::new(),
            delta_count: 0,
            removed: HashSet::new(),
            memory_base,
            postings_file: None,
            cache: HashMap::new(),
            cache_order: Vec::new(),
            document_count: 0,
        }
    }

    pub fn document_count(&self) -> usize {
        self.document_count
    }

    fn extract(&self, document: &Value) -> String {
        if let Some(fields) = &self.fields
            && !fields.is_empty()
        {
            return fields
                .iter()
                .filter_map(|field| get_path(document, field)?.as_str())
                .collect::<Vec<_>>()
                .join(" ");
        }
        let mut leaves = Vec::new();
        string_leaves(document, &mut leaves);
        leaves.join(" ")
    }

    pub fn term_count(&self) -> usize {
        if let Some(memory) = &self.memory_base {
            memory.len()
                + self
                    .delta
                    .keys()
                    .filter(|term| !memory.contains_key(*term))
                    .count()
        } else {
            self.postings.len()
                + self
                    .delta
                    .keys()
                    .filter(|term| !self.postings.contains_key(*term))
                    .count()
        }
    }

    // Original: packages/minidb/src/text-index.ts, TextIndex.build().
    pub fn build<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a str, &'a Value)>,
    ) -> Result<(), TextIndexError> {
        let mut aggregate = HashMap::<String, BTreeMap<u32, u32>>::new();
        let mut new_keys = Vec::new();
        let mut new_key_to_id = HashMap::new();
        let mut new_lengths = HashMap::new();
        for (key, value) in entries {
            let document_id = new_keys.len() as u32;
            new_keys.push(Some(key.to_owned()));
            new_key_to_id.insert(key.to_owned(), document_id);
            let tokens = tokenize(&self.extract(value));
            let mut counts = HashMap::<String, u32>::new();
            for term in &tokens {
                *counts.entry(term.clone()).or_default() += 1;
            }
            for (term, frequency) in counts {
                aggregate
                    .entry(term)
                    .or_default()
                    .insert(document_id, frequency);
            }
            new_lengths.insert(document_id, tokens.len());
        }

        if let Some(path) = &self.path {
            let had_old = self.postings_file.take().is_some();
            let staged = aggregate
                .iter()
                .map(|(term, values)| {
                    (
                        term.clone(),
                        values
                            .iter()
                            .map(|(&id, &frequency)| (id, frequency))
                            .collect(),
                    )
                })
                .collect::<Vec<_>>();
            let dictionary = match PostingsFile::rebuild(path, staged) {
                Ok(dictionary) => dictionary,
                Err(error) => {
                    if had_old {
                        self.postings_file = PostingsFile::open(path).ok();
                    }
                    return Err(error.into());
                }
            };
            let new_file = PostingsFile::open(path)?;
            self.postings = dictionary;
            self.postings_file = Some(new_file);
        } else {
            self.memory_base = Some(aggregate);
        }

        self.document_count = new_keys.len();
        self.document_lengths = new_lengths;
        self.keys = new_keys;
        self.key_to_id = new_key_to_id;
        self.delta.clear();
        self.delta_count = 0;
        self.removed.clear();
        self.cache.clear();
        self.cache_order.clear();
        Ok(())
    }

    // Original: TextIndex.add(). Replacing a key tombstones its previous doc ID.
    pub fn add(&mut self, key: &str, document: &Value) {
        if self.key_to_id.contains_key(key) {
            self.remove(key);
        }
        let document_id = self.keys.len() as u32;
        self.keys.push(Some(key.to_owned()));
        self.key_to_id.insert(key.to_owned(), document_id);
        let tokens = tokenize(&self.extract(document));
        let mut counts = HashMap::<String, u32>::new();
        for term in &tokens {
            *counts.entry(term.clone()).or_default() += 1;
        }
        for (term, frequency) in counts {
            self.delta
                .entry(term)
                .or_default()
                .insert(document_id, frequency);
            self.delta_count += 1;
        }
        self.document_lengths.insert(document_id, tokens.len());
        self.document_count += 1;
    }

    pub fn remove(&mut self, key: &str) {
        let Some(document_id) = self.key_to_id.remove(key) else {
            return;
        };
        self.removed.insert(document_id);
        self.keys[document_id as usize] = None;
        self.document_lengths.remove(&document_id);
        for values in self.delta.values_mut() {
            if values.remove(&document_id).is_some() {
                self.delta_count = self.delta_count.saturating_sub(1);
            }
        }
        self.document_count = self.document_count.saturating_sub(1);
    }

    fn read_base(&mut self, term: &str) -> Result<BTreeMap<u32, u32>, TextIndexError> {
        if let Some(base) = &self.memory_base {
            return Ok(base.get(term).cloned().unwrap_or_default());
        }
        let entries = if let Some(entries) = self.cache.get(term).cloned() {
            self.touch_cache(term);
            entries
        } else {
            let entries = match (
                self.postings.get(term).copied(),
                self.postings_file.as_mut(),
            ) {
                (Some(entry), Some(file)) => file.read(entry)?,
                _ => Vec::new(),
            };
            if self.cache_terms > 0 {
                self.insert_cache(term, entries.clone());
            }
            entries
        };
        Ok(entries.into_iter().collect())
    }

    fn touch_cache(&mut self, term: &str) {
        self.cache_order.retain(|existing| existing != term);
        self.cache_order.push(term.to_owned());
    }

    fn insert_cache(&mut self, term: &str, entries: Vec<(u32, u32)>) {
        self.cache.insert(term.to_owned(), entries);
        self.touch_cache(term);
        if self.cache.len() > self.cache_terms
            && let Some(oldest) = self.cache_order.first().cloned()
        {
            self.cache_order.remove(0);
            self.cache.remove(&oldest);
        }
    }

    fn live_postings(&mut self, term: &str) -> Result<BTreeMap<u32, u32>, TextIndexError> {
        let mut output = self.read_base(term)?;
        output.retain(|document_id, _| !self.removed.contains(document_id));
        if let Some(delta) = self.delta.get(term) {
            for (&document_id, &frequency) in delta {
                if !self.removed.contains(&document_id) {
                    output.insert(document_id, frequency);
                }
            }
        }
        Ok(output)
    }

    // Original: TextIndex.search(). TF-IDF formula and AND/OR candidate behavior are preserved.
    pub fn search(
        &mut self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, TextIndexError> {
        let mut seen = HashSet::new();
        let terms = tokenize(query)
            .into_iter()
            .filter(|term| seen.insert(term.clone()))
            .collect::<Vec<_>>();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut term_maps = HashMap::new();
        for term in &terms {
            term_maps.insert(term.clone(), self.live_postings(term)?);
        }
        let mut candidates = BTreeSet::new();
        if options.operator == SearchOperator::Or {
            for postings in term_maps.values() {
                candidates.extend(postings.keys().copied());
            }
        } else {
            let mut lists = term_maps.values().collect::<Vec<_>>();
            if lists.iter().any(|list| list.is_empty()) {
                return Ok(Vec::new());
            }
            lists.sort_by_key(|list| list.len());
            candidates.extend(lists[0].keys().copied());
            for list in &lists[1..] {
                candidates.retain(|id| list.contains_key(id));
            }
        }
        let mut scored = Vec::new();
        for document_id in candidates {
            let length = self
                .document_lengths
                .get(&document_id)
                .copied()
                .unwrap_or(1) as f64;
            let mut score = 0.0;
            for term in &terms {
                let postings = &term_maps[term];
                let frequency = postings.get(&document_id).copied().unwrap_or(0) as f64;
                if frequency > 0.0 {
                    score += (frequency / length)
                        * (1.0 + self.document_count as f64 / postings.len().max(1) as f64).ln();
                }
            }
            if score > 0.0
                && let Some(Some(key)) = self.keys.get(document_id as usize)
            {
                scored.push(SearchHit {
                    key: key.clone(),
                    score,
                });
            }
        }
        scored.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(options.limit);
        Ok(scored)
    }

    pub fn close(&mut self) {
        self.postings_file = None;
    }
}

// Original: text-index.ts, tokenize().
pub fn tokenize(input: &str) -> Vec<String> {
    static LATIN: OnceLock<Regex> = OnceLock::new();
    static CJK: OnceLock<Regex> = OnceLock::new();
    let lower = input.to_lowercase();
    let mut terms = LATIN
        .get_or_init(|| Regex::new(r"[a-z0-9]+").expect("valid latin regex"))
        .find_iter(&lower)
        .map(|item| item.as_str())
        .filter(|term| term.len() <= MAX_TERM_BYTES)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for run in CJK
        .get_or_init(|| {
            Regex::new(r"[\u{3400}-\u{9fff}\u{3040}-\u{30ff}\u{ff00}-\u{ffef}]+")
                .expect("valid CJK regex")
        })
        .find_iter(&lower)
    {
        let characters = run.as_str().chars().collect::<Vec<_>>();
        for index in 0..characters.len() {
            terms.push(characters[index].to_string());
            if let Some(next) = characters.get(index + 1) {
                terms.push(format!("{}{next}", characters[index]));
            }
        }
    }
    terms
}

fn string_leaves<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
    match value {
        Value::String(value) => output.push(value),
        Value::Array(values) => {
            for value in values {
                string_leaves(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                string_leaves(value, output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_latin_and_cjk() {
        assert_eq!(tokenize("Hello, 世界"), vec!["hello", "世", "世界", "界"]);
    }

    #[test]
    fn builds_updates_and_searches_memory_index() {
        let mut index = TextIndex::new(TextIndexOptions::default());
        let first = serde_json::json!({"text":"rust database"});
        let second = serde_json::json!({"text":"rust rust code"});
        index.build([("a", &first), ("b", &second)]).unwrap();
        assert_eq!(
            index.search("database", &SearchOptions::default()).unwrap()[0].key,
            "a"
        );
        index.remove("a");
        assert!(
            index
                .search("database", &SearchOptions::default())
                .unwrap()
                .is_empty()
        );
        index.add("c", &serde_json::json!({"text":"database code"}));
        assert_eq!(
            index.search("database", &SearchOptions::default()).unwrap()[0].key,
            "c"
        );
    }

    #[test]
    fn rebuilds_and_searches_disk_index() {
        let directory = tempfile::tempdir().unwrap();
        let mut index = TextIndex::new(TextIndexOptions {
            postings_path: Some(directory.path().join("postings")),
            ..Default::default()
        });
        let document = serde_json::json!({"text":"searchable"});
        index.build([("key", &document)]).unwrap();
        assert_eq!(
            index
                .search("searchable", &SearchOptions::default())
                .unwrap()[0]
                .key,
            "key"
        );
    }
}
