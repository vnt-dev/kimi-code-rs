//! Indexed, queryable derived read-model contract.
//!
//! Original: `packages/agent-core-v2/src/persistence/interface/queryStore.ts`.
//!
//! The concrete migration uses the existing `kimi-code-minidb` crate; this
//! module intentionally defines only the engine-independent public contract.

use std::{error::Error, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ComparisonOp {
    #[serde(rename = "$eq", default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<Value>,
    #[serde(rename = "$ne", default, skip_serializing_if = "Option::is_none")]
    pub ne: Option<Value>,
    #[serde(rename = "$gt", default, skip_serializing_if = "Option::is_none")]
    pub gt: Option<Value>,
    #[serde(rename = "$gte", default, skip_serializing_if = "Option::is_none")]
    pub gte: Option<Value>,
    #[serde(rename = "$lt", default, skip_serializing_if = "Option::is_none")]
    pub lt: Option<Value>,
    #[serde(rename = "$lte", default, skip_serializing_if = "Option::is_none")]
    pub lte: Option<Value>,
    #[serde(rename = "$in", default, skip_serializing_if = "Option::is_none")]
    pub r#in: Option<Vec<Value>>,
    #[serde(rename = "$nin", default, skip_serializing_if = "Option::is_none")]
    pub nin: Option<Vec<Value>>,
    #[serde(rename = "$exists", default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
}

pub type QueryFilter = Map<String, Value>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum IndexDef {
    Value {
        name: String,
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unique: Option<bool>,
    },
    Compound {
        name: String,
        #[serde(rename = "groupBy")]
        group_by: String,
        #[serde(rename = "orderBy")]
        order_by: String,
    },
    Text {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WriteOp {
    Put {
        collection: String,
        key: String,
        value: Value,
    },
    Delete {
        collection: String,
        key: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Checkpoint {
    pub seq: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum QueryStoreError {
    #[error("query-store value conversion failed")]
    Codec(#[source] serde_json::Error),

    #[error("query-store backend failed: {0}")]
    Backend(#[source] Box<dyn Error + Send + Sync>),
}

impl QueryStoreError {
    pub fn backend(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}

#[async_trait]
pub trait QueryBuilderService: Send {
    fn where_filter(&mut self, filter: QueryFilter);
    fn order_by(&mut self, field: String, direction: SortDir);
    fn limit(&mut self, limit: u64);
    fn cursor(&mut self, cursor: Option<String>);
    async fn execute_values(&self) -> Result<Page<Value>, QueryStoreError>;
}

pub struct Query<T> {
    inner: Box<dyn QueryBuilderService>,
    marker: PhantomData<fn() -> T>,
}

impl<T> Query<T> {
    fn new(inner: Box<dyn QueryBuilderService>) -> Self {
        Self {
            inner,
            marker: PhantomData,
        }
    }

    // Original: IQuery.where(). Named `where_filter` because `where` is a Rust keyword.
    pub fn where_filter(&mut self, filter: QueryFilter) -> &mut Self {
        self.inner.where_filter(filter);
        self
    }

    pub fn order_by(&mut self, field: impl Into<String>, direction: SortDir) -> &mut Self {
        self.inner.order_by(field.into(), direction);
        self
    }

    pub fn limit(&mut self, limit: u64) -> &mut Self {
        self.inner.limit(limit);
        self
    }

    pub fn cursor(&mut self, cursor: Option<String>) -> &mut Self {
        self.inner.cursor(cursor);
        self
    }
}

impl<T: DeserializeOwned> Query<T> {
    pub async fn execute(&self) -> Result<Page<T>, QueryStoreError> {
        let page = self.inner.execute_values().await?;
        let items = page
            .items
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(QueryStoreError::Codec)?;
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }
}

#[async_trait]
pub trait QueryStoreService: Send + Sync {
    async fn put_value(
        &self,
        collection: &str,
        key: &str,
        value: Value,
    ) -> Result<(), QueryStoreError>;
    async fn batch(&self, operations: &[WriteOp]) -> Result<(), QueryStoreError>;
    async fn delete(&self, collection: &str, key: &str) -> Result<(), QueryStoreError>;
    async fn get_value(
        &self,
        collection: &str,
        key: &str,
    ) -> Result<Option<Value>, QueryStoreError>;
    fn query_values(&self, collection: &str) -> Box<dyn QueryBuilderService>;
    async fn ensure_index(
        &self,
        collection: &str,
        definition: &IndexDef,
    ) -> Result<(), QueryStoreError>;
    async fn get_checkpoint(&self, source: &str) -> Result<Option<Checkpoint>, QueryStoreError>;
    async fn set_checkpoint(
        &self,
        source: &str,
        checkpoint: Checkpoint,
    ) -> Result<(), QueryStoreError>;
    async fn close(&self) -> Result<(), QueryStoreError>;
}

#[derive(Clone)]
pub struct QueryStoreHandle(pub Arc<dyn QueryStoreService>);

impl QueryStoreHandle {
    pub async fn put<T: Serialize>(
        &self,
        collection: &str,
        key: &str,
        value: &T,
    ) -> Result<(), QueryStoreError> {
        let value = serde_json::to_value(value).map_err(QueryStoreError::Codec)?;
        self.0.put_value(collection, key, value).await
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        collection: &str,
        key: &str,
    ) -> Result<Option<T>, QueryStoreError> {
        self.0
            .get_value(collection, key)
            .await?
            .map(|value| serde_json::from_value(value).map_err(QueryStoreError::Codec))
            .transpose()
    }

    pub fn query<T>(&self, collection: &str) -> Query<T> {
        Query::new(self.0.query_values(collection))
    }
}

pub const QUERY_STORE_SERVICE_ID: ServiceIdentifier<QueryStoreHandle> =
    ServiceIdentifier::new("queryStore");

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct StubQuery {
        filter: Mutex<QueryFilter>,
    }

    #[async_trait]
    impl QueryBuilderService for StubQuery {
        fn where_filter(&mut self, filter: QueryFilter) {
            self.filter.lock().unwrap().extend(filter);
        }

        fn order_by(&mut self, _field: String, _direction: SortDir) {}
        fn limit(&mut self, _limit: u64) {}
        fn cursor(&mut self, _cursor: Option<String>) {}

        async fn execute_values(&self) -> Result<Page<Value>, QueryStoreError> {
            Ok(Page {
                items: vec![serde_json::json!({"name": "kimi"})],
                next_cursor: Some("1".into()),
            })
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct Row {
        name: String,
    }

    #[tokio::test]
    async fn typed_query_preserves_builder_and_page_conversion() {
        let mut query = Query::<Row>::new(Box::new(StubQuery {
            filter: Mutex::new(QueryFilter::new()),
        }));
        query
            .where_filter(Map::from_iter([("active".into(), Value::Bool(true))]))
            .order_by("name", SortDir::Desc)
            .limit(1)
            .cursor(Some("0".into()));
        assert_eq!(
            query.execute().await.unwrap(),
            Page {
                items: vec![Row {
                    name: "kimi".into()
                }],
                next_cursor: Some("1".into())
            }
        );
    }

    #[test]
    fn external_shapes_and_identifier_match_source_contract() {
        let comparison = ComparisonOp {
            gte: Some(Value::from(3)),
            r#in: Some(vec![Value::from(3), Value::from(4)]),
            ..ComparisonOp::default()
        };
        assert_eq!(
            serde_json::to_value(comparison).unwrap(),
            serde_json::json!({"$gte": 3, "$in": [3, 4]})
        );
        assert_eq!(
            serde_json::to_value(IndexDef::Compound {
                name: "by_time".into(),
                group_by: "session_id".into(),
                order_by: "created_at".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "compound",
                "name": "by_time",
                "groupBy": "session_id",
                "orderBy": "created_at"
            })
        );
        assert_eq!(QUERY_STORE_SERVICE_ID.to_string(), "queryStore");
    }
}
