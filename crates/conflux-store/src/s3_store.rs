//! `S3Store` — a `Store` backend against object storage.
//!
//! Configured against a custom endpoint (MinIO in this crate's own tests,
//! but any S3-compatible service works the same way) rather than assuming
//! real AWS — connection details are passed directly as arguments by
//! whatever constructs this store, the same as `PostgresStore`'s
//! `postgres_url`; this crate does not itself read config or env vars.

use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;

use crate::{Store, StoreError};

const DEFAULT_PREFIX: &str = "conflux";

pub struct S3Store {
    client: Client,
    bucket: String,
    prefix: String,
}

impl S3Store {
    pub async fn connect(
        endpoint_url: &str,
        bucket: impl Into<String>,
        access_key: &str,
        secret_key: &str,
    ) -> Result<Self, StoreError> {
        Self::connect_with_prefix(endpoint_url, bucket, access_key, secret_key, DEFAULT_PREFIX)
            .await
    }

    /// Lets multiple independent stores share one bucket under different
    /// key prefixes — same reason `PostgresStore`/`RedisRegistry` support
    /// per-instance tables/keys: this crate's own tests need per-test
    /// isolation against one real, never-wiped bucket.
    pub async fn connect_with_prefix(
        endpoint_url: &str,
        bucket: impl Into<String>,
        access_key: &str,
        secret_key: &str,
        prefix: impl Into<String>,
    ) -> Result<Self, StoreError> {
        let bucket = bucket.into();
        let prefix = prefix.into();

        let credentials = Credentials::new(access_key, secret_key, None, None, "conflux-store");
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint_url)
            .region(Region::new("us-east-1"))
            .credentials_provider(credentials)
            // Required by most self-hosted S3-compatible services,
            // including MinIO — virtual-hosted-style bucket addressing
            // needs real DNS wildcarding this test setup doesn't have.
            .force_path_style(true)
            .build();
        let client = Client::from_conf(config);

        // Idempotent: MinIO returns a "bucket already owned by you" error
        // on a second create, which is fine — this is just "ensure it
        // exists," not "create fresh."
        let _ = client.create_bucket().bucket(&bucket).send().await;

        Ok(Self {
            client,
            bucket,
            prefix,
        })
    }

    fn key_for(&self, round: u64) -> String {
        format!("{}/checkpoint-{round}.bin", self.prefix)
    }

    fn key_prefix(&self) -> String {
        format!("{}/checkpoint-", self.prefix)
    }
}

impl Store for S3Store {
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        let list_prefix = self.key_prefix();
        let response = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&list_prefix)
            .send()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let mut latest_round: Option<u64> = None;
        for object in response.contents() {
            if let Some(round) = object
                .key()
                .and_then(|key| key.strip_prefix(&list_prefix))
                .and_then(|s| s.strip_suffix(".bin"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                latest_round = Some(latest_round.map_or(round, |current| current.max(round)));
            }
        }

        let round = latest_round.ok_or(StoreError::NoCheckpoint)?;
        let key = self.key_for(round);

        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let bytes = object
            .body
            .collect()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .into_bytes();

        if !bytes.len().is_multiple_of(4) {
            return Err(StoreError::MalformedCheckpoint {
                path: key,
                len: bytes.len(),
            });
        }
        Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        // S3's PutObject is natively overwrite-if-exists — a retried
        // round (the same "flushed twice under a race" case
        // `PostgresStore` handles with `ON CONFLICT`) just overwrites the
        // existing object, no explicit upsert logic needed here.
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.key_for(round))
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `docker run -d --name conflux-dev-minio -p 19000:9000 -p 19001:9001
    /// -e MINIO_ROOT_USER=confluxadmin -e MINIO_ROOT_PASSWORD=confluxsecret
    /// minio/minio server /data --console-address ":9001"`
    const TEST_ENDPOINT: &str = "http://127.0.0.1:19000";
    const TEST_BUCKET: &str = "conflux-test-bucket";
    const TEST_ACCESS_KEY: &str = "confluxadmin";
    const TEST_SECRET_KEY: &str = "confluxsecret";

    fn unique_prefix(test_name: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("test-{test_name}-{}-{n}", std::process::id())
    }

    async fn connect(test_name: &str) -> S3Store {
        S3Store::connect_with_prefix(
            TEST_ENDPOINT,
            TEST_BUCKET,
            TEST_ACCESS_KEY,
            TEST_SECRET_KEY,
            unique_prefix(test_name),
        )
        .await
        .expect("connect to the dev MinIO container — is it running?")
    }

    #[tokio::test]
    async fn round_trips_through_object_storage() {
        let store = connect("round_trips").await;

        store.save_checkpoint(1, &[1.5, -2.5, 3.0]).await.unwrap();

        assert_eq!(
            store.load_latest_weights().await.unwrap(),
            vec![1.5, -2.5, 3.0]
        );
    }

    #[tokio::test]
    async fn loads_the_highest_round() {
        let store = connect("highest_round").await;

        store.save_checkpoint(1, &[1.0]).await.unwrap();
        store.save_checkpoint(3, &[3.0]).await.unwrap();
        store.save_checkpoint(2, &[2.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0]);
    }

    #[tokio::test]
    async fn errors_on_no_checkpoint() {
        let store = connect("no_checkpoint").await;

        let err = store.load_latest_weights().await.unwrap_err();

        assert!(matches!(err, StoreError::NoCheckpoint));
    }

    #[tokio::test]
    async fn saving_the_same_round_twice_overwrites_instead_of_erroring() {
        let store = connect("overwrite").await;

        store.save_checkpoint(1, &[1.0]).await.unwrap();
        store.save_checkpoint(1, &[9.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![9.0]);
    }
}
