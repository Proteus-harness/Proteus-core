use super::{Backend, RemoteCacheSource, cache};
use serde_json::{Value, json};

fn digest(value: &Value) -> String {
    proteus_contracts::contracts::storage_value_etag(value).unwrap()
}
fn key(digest: &str) -> String {
    format!("display-data-{digest}")
}

#[test]
fn physical_packs_preserve_logical_leaves_and_skip_the_latest_window() {
    let (backend, store) = Backend::store();
    let source = RemoteCacheSource::read(&store).unwrap();
    let rows: Vec<_> = (0..953)
        .map(|n| format!("row-{n} {}", "repeat".repeat(8)))
        .collect();
    let section = cache::write_transcript(&store, &source, &rows).unwrap();
    let manifest = serde_json::to_value(&section).unwrap();
    let packs = manifest["packs"].as_array().unwrap();
    assert_eq!(
        packs
            .iter()
            .map(|p| p["count"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [8, 8, 2]
    );
    for (page, rows) in manifest["pages"]
        .as_array()
        .unwrap()
        .iter()
        .zip(rows.chunks(50))
    {
        assert_eq!(page["hash"], digest(&json!(rows)));
    }
    let pack_keys: Vec<_> = packs
        .iter()
        .map(|p| key(p["hash"].as_str().unwrap()))
        .collect();
    {
        let mut state = backend.0.lock().unwrap();
        assert_eq!(
            state.writes.len(),
            5,
            "three packs and two unaligned tail leaves"
        );
        state.cache_reads.clear();
    }
    assert_eq!(
        cache::read_section::<String>(&store, &section, 903, 953).unwrap(),
        rows[903..]
    );
    {
        let mut state = backend.0.lock().unwrap();
        assert_eq!(state.cache_reads.len(), 2);
        assert!(state.cache_reads.iter().all(|key| !pack_keys.contains(key)));
        state.cache_reads.clear();
    }
    assert_eq!(
        cache::read_section::<String>(&store, &section, 395, 445).unwrap(),
        rows[395..445]
    );
    assert_eq!(backend.0.lock().unwrap().cache_reads, pack_keys[..2]);
    backend.0.lock().unwrap().cache_reads.clear();
    assert_eq!(
        cache::read_section::<String>(&store, &section, 0, 953).unwrap(),
        rows
    );
    assert_eq!(backend.0.lock().unwrap().cache_reads.len(), 5);
    {
        let mut state = backend.0.lock().unwrap();
        state.cache_reads.clear();
        state.writes.clear();
    }
    let next = cache::append_section(&store, &source, &section, &[String::from("added")]).unwrap();
    {
        let state = backend.0.lock().unwrap();
        assert!(
            state
                .cache_reads
                .iter()
                .chain(&state.writes)
                .all(|key| !pack_keys.contains(key))
        );
    }
    assert_eq!(
        cache::read_section::<String>(&store, &next, 953, 954).unwrap(),
        ["added"]
    );
    assert_eq!(
        cache::read_section::<String>(&store, &section, 950, 953).unwrap(),
        rows[950..]
    );
    let singletons = cache::write_section(&store, &source, &rows).unwrap();
    let singletons = serde_json::to_value(singletons).unwrap();
    assert!(singletons.get("packs").is_none());
    assert_eq!(singletons["pages"], manifest["pages"]);
    let singletons = serde_json::from_value(singletons).unwrap();
    assert_eq!(
        cache::read_section::<String>(&store, &singletons, 0, 953).unwrap(),
        rows
    );
}

#[test]
fn invalid_archive_objects_and_spans_are_cache_misses_not_mixed_rows() {
    let (backend, store) = Backend::store();
    let source = RemoteCacheSource::read(&store).unwrap();
    let rows: Vec<_> = (0..953).collect::<Vec<u32>>();
    let section = cache::write_transcript(&store, &source, &rows).unwrap();
    let manifest = serde_json::to_value(&section).unwrap();
    assert_eq!(
        cache::read_section::<u32>(&store, &section, 394, 444).unwrap(),
        rows[394..444]
    );
    let pack_key = key(manifest["packs"][0]["hash"].as_str().unwrap());
    let mut bad = manifest.clone();
    bad["pages"][0]["hash"] = bad["pages"][1]["hash"].clone();
    let bad = serde_json::from_value(bad).unwrap();
    assert!(cache::read_section::<u32>(&store, &bad, 0, 1).is_none());
    for (i, first, count) in [(0, 0u64, 9), (1, 7, 8), (2, 17, 2), (0, u64::MAX, 2)] {
        let mut bad = manifest.clone();
        bad["packs"][i]["first"] = json!(first);
        bad["packs"][i]["count"] = json!(count);
        // A target with narrower usize rejects the index during deserialization.
        if let Ok(bad) = serde_json::from_value(bad) {
            assert!(cache::read_section::<u32>(&store, &bad, 903, 953).is_none());
        }
    }
    let mut body = backend.0.lock().unwrap().cache.remove(&pack_key).unwrap();
    assert!(cache::read_section::<u32>(&store, &section, 0, 1).is_none());
    assert_eq!(
        cache::read_section::<u32>(&store, &section, 903, 953).unwrap(),
        rows[903..]
    );
    // A hash-consistent object in the superseded unframed shape is a miss.
    body = json!([rows[..50].to_vec()]);
    let mut bad = manifest.clone();
    bad["packs"][0]["hash"] = json!(digest(&body));
    backend
        .0
        .lock()
        .unwrap()
        .cache
        .insert(key(&digest(&body)), body);
    let bad = serde_json::from_value(bad).unwrap();
    assert!(cache::read_section::<u32>(&store, &bad, 0, 1).is_none());
}

#[test]
fn historical_packs_obey_actual_json_byte_cap_and_large_leaf_remains_readable() {
    let (backend, store) = Backend::store();
    let source = RemoteCacheSource::read(&store).unwrap();
    let mut rows: Vec<_> = (0..600)
        .map(|n| format!("{}-{n}", "x".repeat(1700)))
        .collect();
    rows[260] = "x".repeat(300 * 1024);
    let section = cache::write_transcript(&store, &source, &rows).unwrap();
    let manifest = serde_json::to_value(&section).unwrap();
    let state = backend.0.lock().unwrap();
    for pack in manifest["packs"].as_array().unwrap() {
        let bytes = &state.cache[&key(pack["hash"].as_str().unwrap())];
        assert!(serde_json::to_vec(bytes).unwrap().len() <= 256 * 1024);
        assert!((2..=3).contains(&pack["count"].as_u64().unwrap()));
    }
    let large = &manifest["pages"][5];
    assert!(large["bytes"].as_u64().unwrap() > 256 * 1024);
    assert!(
        state
            .cache
            .contains_key(&key(large["hash"].as_str().unwrap()))
    );
    drop(state);
    assert_eq!(
        cache::read_section::<String>(&store, &section, 248, 268).unwrap(),
        rows[248..268]
    );
}

#[test]
fn framed_pack_reads_recheck_backing_and_bound_logical_leaf_bytes() {
    let (backend, store) = Backend::store();
    let source = RemoteCacheSource::read(&store).unwrap();
    let rows: Vec<_> = (0..953)
        .map(|n| format!("row-{n} {}", "repeat".repeat(32)))
        .collect();
    let section = cache::write_transcript(&store, &source, &rows).unwrap();
    let manifest = serde_json::to_value(&section).unwrap();
    let pack = &manifest["packs"][0];
    let physical_key = key(pack["hash"].as_str().unwrap());
    assert_eq!(
        cache::read_section::<String>(&store, &section, 0, 1).unwrap(),
        rows[..1]
    );
    let original = backend
        .0
        .lock()
        .unwrap()
        .cache
        .remove(&physical_key)
        .unwrap();
    assert!(cache::read_section::<String>(&store, &section, 0, 1).is_none());
    let mut swapped = original.clone();
    swapped["deflate_v1"].as_array_mut().unwrap().swap(0, 1);
    backend
        .0
        .lock()
        .unwrap()
        .cache
        .insert(physical_key.clone(), swapped.clone());
    assert!(cache::read_section::<String>(&store, &section, 0, 1).is_none());
    let swapped_hash = digest(&swapped);
    backend
        .0
        .lock()
        .unwrap()
        .cache
        .insert(key(&swapped_hash), swapped);
    let mut changed = manifest.clone();
    changed["packs"][0]["hash"] = json!(swapped_hash);
    let changed = serde_json::from_value(changed).unwrap();
    assert!(cache::read_section::<String>(&store, &changed, 0, 1).is_none());

    backend
        .0
        .lock()
        .unwrap()
        .cache
        .insert(physical_key, original);
    let mut shorter = manifest;
    shorter["pages"][0]["bytes"] = json!(shorter["pages"][0]["bytes"].as_u64().unwrap() - 1);
    let shorter = serde_json::from_value(shorter).unwrap();
    assert!(cache::read_section::<String>(&store, &shorter, 0, 1).is_none());
}
