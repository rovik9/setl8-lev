mod common;
use common::*;
use journal_types::*;

#[test]
fn vector_genesis_hash() {
    let v = vectors();
    assert_eq!(v["genesis"]["pubkey"].as_str().unwrap(), hex(&PUBKEY_RANGE));
    assert_eq!(hex(&genesis()), v["genesis"]["hash"].as_str().unwrap());
}

#[test]
fn vector_every_event_encodes_to_the_python_bytes() {
    let v = vectors();
    for name in EVENT_ORDER {
        let want = v["events"][name]["bytes"].as_str().unwrap();
        assert_eq!(hex(&event(name).to_bytes()), want, "{name}");
    }
}

#[test]
fn vector_every_event_decodes_from_the_python_bytes() {
    let v = vectors();
    for name in EVENT_ORDER {
        let bytes = unhex(v["events"][name]["bytes"].as_str().unwrap());
        assert_eq!(JournalEvent::decode(&bytes).unwrap(), event(name), "{name}");
    }
}

#[test]
fn vector_noproof_variants_encode_to_the_python_bytes() {
    let v = vectors();
    for name in ["Fill", "Close", "TightStart", "FloorRaised", "Breach", "Fail"] {
        let want = v["events"][name]["bytes_noproof"].as_str().unwrap();
        assert_eq!(hex(&without_proof(&event(name)).to_bytes()), want, "{name}");
    }
}

#[test]
fn vector_price_proofs_for_sources_2_3_4_255() {
    let v = vectors();
    for name in ["src2", "src3", "src4", "src255"] {
        let p = proof(name);
        let bytes = p.to_bytes().unwrap();
        assert_eq!(hex(&bytes), v["proofs"][name]["bytes"].as_str().unwrap(), "{name} bytes");
        assert_eq!(PriceProof::decode(&bytes).unwrap(), p, "{name} decode");
        assert_eq!(hex(&proof_digest(&sha(), &p).unwrap()), v["proofs"][name]["digest"].as_str().unwrap(), "{name} digest");
    }
}

#[test]
fn vector_full_batch_of_three_events_and_two_proofs() {
    let v = vectors();
    let want = unhex(v["batch3"]["bytes"].as_str().unwrap());
    assert_eq!(batch3().to_bytes().unwrap(), want);
    assert_eq!(JournalBatch::decode(&want).unwrap(), batch3());
}

#[test]
fn vector_next_hash_for_each_event_with_and_without_a_proof() {
    let v = vectors();
    let proofs = proof_list();
    for (k, name) in EVENT_ORDER.iter().enumerate() {
        let seq = BASE_SEQ + k as u64;
        let ev = event(name);
        let entry = &v["events"][*name];
        assert_eq!(entry["seq"].as_u64().unwrap(), seq, "{name} seq");
        match ev.proof_index() {
            Some(idx) => {
                let got = next_hash(&sha(), &genesis(), seq, &ev, Some(&proofs[idx])).unwrap();
                assert_eq!(hex(&got), entry["next_hash_with_proof"].as_str().unwrap(), "{name} with proof");
                let got = next_hash(&sha(), &genesis(), seq, &without_proof(&ev), None).unwrap();
                assert_eq!(hex(&got), entry["next_hash_noproof"].as_str().unwrap(), "{name} without proof");
            }
            None => {
                let got = next_hash(&sha(), &genesis(), seq, &ev, None).unwrap();
                assert_eq!(hex(&got), entry["next_hash"].as_str().unwrap(), "{name}");
            }
        }
    }
}

#[test]
fn vector_three_event_chain_via_apply_batch_and_verify_chain() {
    let v = vectors();
    let hashes: Vec<[u8; 32]> =
        v["batch3"]["hashes"].as_array().unwrap().iter().map(|h| unhex32(h.as_str().unwrap())).collect();
    let b = batch3();
    let (last, last_seq) = apply_batch(&sha(), &b.prev_hash, 1, &b).unwrap();
    assert_eq!((last, last_seq), (hashes[2], 3));

    let entries: Vec<ChainEntry> = b
        .events
        .iter()
        .enumerate()
        .map(|(i, e)| ChainEntry {
            seq: 1 + i as u64,
            event: e.clone(),
            proof: e.proof_index().map(|idx| b.proofs[idx].clone()),
            hash: hashes[i],
        })
        .collect();
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &entries), Ok(hashes[2]));
}

#[test]
fn vector_feed_ids() {
    let v = vectors();
    let list = v["feed_ids"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    for item in list {
        let source = u8::try_from(item["source"].as_u64().unwrap()).unwrap();
        let symbol = item["symbol"].as_str().unwrap();
        assert_eq!(hex(&feed_id(&sha(), source, symbol)), item["feed_id"].as_str().unwrap(), "{symbol}");
    }
    // the three requested cases are present
    let pairs: Vec<(u64, &str)> = list.iter().map(|i| (i["source"].as_u64().unwrap(), i["symbol"].as_str().unwrap())).collect();
    assert_eq!(pairs, vec![(2, "BTC"), (4, "xyz:AAPL"), (0, "EURUSD")]);
}

#[test]
fn feed_id_depends_on_source_and_symbol() {
    assert_ne!(feed(2, "BTC"), feed(3, "BTC"));
    assert_ne!(feed(2, "BTC"), feed(2, "ETH"));
    assert_eq!(feed(2, "BTC"), feed(2, "BTC"));
}

#[test]
fn an_injected_closure_hasher_gives_the_same_results_as_the_native_one() {
    let calls = std::cell::Cell::new(0u32);
    let closure = |parts: &[&[u8]]| -> [u8; 32] {
        calls.set(calls.get() + 1);
        NativeSha256.hashv(parts)
    };
    let b = batch3();
    let native = apply_batch(&sha(), &b.prev_hash, 1, &b).unwrap();
    let injected = apply_batch(&closure, &b.prev_hash, 1, &b).unwrap();
    assert_eq!(native, injected);
    assert!(calls.get() >= 5, "3 events + 2 proof digests = 5 hashes, got {}", calls.get());
    assert_eq!(genesis_hash(&closure, &PUBKEY_RANGE), genesis());
}
