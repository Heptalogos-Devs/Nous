// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use futures::{StreamExt, stream};
use nous_object_store::ObjectStore;

async fn put(store: &ObjectStore, bytes: &[u8]) -> (String, u64) {
    store
        .put_chunks(stream::iter([Ok(bytes.to_vec())]), 4096)
        .await
        .unwrap()
}

#[tokio::test]
async fn repeat_put_repairs_truncated_and_same_length_corrupt_objects() {
    let root = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(root.path().to_str().unwrap())
        .await
        .unwrap();
    let bytes = b"retained original artifact bytes";
    let identity = put(&store, bytes).await;
    let target = root
        .path()
        .join(ObjectStore::storage_key(&identity.0).unwrap());

    for corrupt in [Vec::new(), vec![b'x'; bytes.len()]] {
        tokio::fs::write(&target, corrupt).await.unwrap();
        assert!(store.get(&identity.0).await.is_err());
        assert_eq!(put(&store, bytes).await, identity);
        assert_eq!(store.get(&identity.0).await.unwrap(), bytes);
        assert_eq!(
            store.read_range(&identity.0, 0, identity.1).await.unwrap(),
            bytes
        );
    }
}

#[tokio::test]
async fn duplicate_and_concurrent_puts_preserve_the_content_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(root.path().to_str().unwrap())
        .await
        .unwrap();
    let bytes = b"one immutable object shared by concurrent registrations";
    let identity = (blake3::hash(bytes).to_hex().to_string(), bytes.len() as u64);
    let results = futures::future::join_all((0..8).map(|_| put(&store, bytes))).await;
    assert!(results.iter().all(|result| result == &identity));
    let target = root
        .path()
        .join(ObjectStore::storage_key(&identity.0).unwrap());
    let modified = tokio::fs::metadata(&target)
        .await
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(put(&store, bytes).await, identity);
    assert_eq!(
        tokio::fs::metadata(&target)
            .await
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );

    tokio::fs::write(&target, []).await.unwrap();
    let results = futures::future::join_all((0..8).map(|_| put(&store, bytes))).await;
    assert!(results.iter().all(|result| result == &identity));
    assert_eq!(store.get(&identity.0).await.unwrap(), bytes);
    assert_eq!(
        store
            .hashes()
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .len(),
        1
    );

    // Zero bytes remain a valid object under their own content identity.
    let empty = put(&store, b"").await;
    assert_eq!(empty.0, blake3::hash(b"").to_hex().to_string());
    assert_eq!(empty.1, 0);
    assert_eq!(put(&store, b"").await, empty);
    assert!(store.get(&empty.0).await.unwrap().is_empty());
}
