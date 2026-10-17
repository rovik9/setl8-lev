mod common;
use common::*;
use session_observer::store::*;

fn read(root: &std::path::Path, date: &str, lane: &str) -> Vec<String> {
    std::fs::read_to_string(LaneWriter::path_for(root, date, lane)).unwrap_or_default().lines().map(String::from).collect()
}

#[test]
fn csv_round_trip_with_a_header() {
    let root = scratch("roundtrip");
    let mut w = LaneWriter::new(&root, "gmx", "recv_ms,symbol,mid");
    w.write(at(0, 10, 0, 0), "1,BTC,81000.5").unwrap();
    w.write(at(0, 10, 0, 1), "2,ETH,2400").unwrap();
    assert_eq!((w.rows, w.bytes), (2, "recv_ms,symbol,mid\n".len() as u64 + "1,BTC,81000.5\n".len() as u64 + "2,ETH,2400\n".len() as u64));
    w.close().unwrap();
    assert_eq!(read(&root, "2026-10-12", "gmx"), vec!["recv_ms,symbol,mid", "1,BTC,81000.5", "2,ETH,2400"]);
}

#[test]
fn rotation_at_midnight_utc() {
    let root = scratch("rotate");
    let mut w = LaneWriter::new(&root, "hl", "h");
    w.write(at(0, 23, 59, 59) + 999, "a").unwrap();
    w.write(at(1, 0, 0, 0), "b").unwrap();
    w.write(at(1, 0, 0, 1), "c").unwrap();
    w.close().unwrap();
    assert_eq!(read(&root, "2026-10-12", "hl"), vec!["h", "a"]);
    assert_eq!(read(&root, "2026-10-13", "hl"), vec!["h", "b", "c"], "a header per file");
}

#[test]
fn restart_appends_and_writes_the_header_once() {
    let root = scratch("restart");
    let mut w = LaneWriter::new(&root, "ref", "h");
    w.write(at(0, 10, 0, 0), "first").unwrap();
    w.close().unwrap();
    let mut w2 = LaneWriter::new(&root, "ref", "h");
    w2.write(at(0, 10, 5, 0), "second").unwrap();
    w2.close().unwrap();
    assert_eq!(read(&root, "2026-10-12", "ref"), vec!["h", "first", "second"]);
    // a lane that never wrote creates nothing
    let w3 = LaneWriter::new(&root, "silent", "h");
    drop(w3);
    assert!(!LaneWriter::path_for(&root, "2026-10-12", "silent").exists());
}

#[test]
fn flushed_every_ten_seconds() {
    let root = scratch("flush");
    let mut w = LaneWriter::new(&root, "x", "h");
    let t = at(0, 10, 0, 0);
    w.write(t, "one").unwrap(); // the first write flushes (nothing was flushed yet)
    assert_eq!(read(&root, "2026-10-12", "x"), vec!["h", "one"]);
    w.write(t + 1_000, "two").unwrap(); // 1 s later: still in the buffer
    assert_eq!(read(&root, "2026-10-12", "x").len(), 2);
    w.tick(t + 9_999).unwrap();
    assert_eq!(read(&root, "2026-10-12", "x").len(), 2, "9.999 s after the last flush");
    w.tick(t + 10_000).unwrap();
    assert_eq!(read(&root, "2026-10-12", "x"), vec!["h", "one", "two"], "10 s after the last flush");
    w.write(t + 12_000, "three").unwrap();
    w.tick(t + 19_999).unwrap();
    assert_eq!(read(&root, "2026-10-12", "x").len(), 3);
    w.tick(t + 20_000).unwrap();
    assert_eq!(read(&root, "2026-10-12", "x").len(), 4);
}

#[test]
fn change_filter_records_only_changes() {
    let mut f = ChangeFilter::new();
    assert!(f.is_empty());
    assert!(f.changed("BTC", "1"), "the first value is a change");
    assert!(!f.changed("BTC", "1"));
    assert!(!f.changed("BTC", "1"));
    assert!(f.changed("BTC", "2"));
    assert!(f.changed("BTC", "1"), "back to an older value is a change");
    assert!(f.changed("ETH", "1"), "keys are independent");
    assert!(!f.changed("ETH", "1"));
    assert_eq!(f.len(), 2);
}

#[test]
fn the_last_heartbeat_and_the_restart_line() {
    let root = scratch("heartbeat");
    assert_eq!(last_heartbeat_ms(&root), None);
    let mut w = LaneWriter::new(&root, "heartbeat", "utc_ms,lane");
    w.write(at(0, 23, 0, 0), &format!("{},gmx", at(0, 23, 0, 0))).unwrap();
    w.write(at(1, 0, 5, 0), &format!("{},gmx", at(1, 0, 5, 0))).unwrap();
    w.write(at(1, 0, 6, 0), &format!("{},hl", at(1, 0, 6, 0))).unwrap();
    w.close().unwrap();
    assert_eq!(last_heartbeat_ms(&root), Some(at(1, 0, 6, 0)), "the newest date, the last row");
    // an unreadable last line is skipped
    std::fs::write(LaneWriter::path_for(&root, "2026-10-13", "heartbeat"), format!("h\n{},gmx\ngarbage\n", at(1, 0, 7, 0))).unwrap();
    assert_eq!(last_heartbeat_ms(&root), Some(at(1, 0, 7, 0)));
    let now = at(1, 0, 17, 0);
    assert!(restart_line(Some(at(1, 0, 7, 0)), now).contains("recording gap 00:10:00"));
    assert!(restart_line(None, now).contains("first start"));
    assert!(restart_line(Some(now + 1), now).contains("in the future"));
    assert!(restart_line(Some(now), now).contains("gap 0 s"));
}

#[test]
fn due_is_inclusive() {
    assert!(due(100, 0, 100));
    assert!(!due(99, 0, 100));
    assert!(due(1_100, 1_000, 100));
    assert!(!due(1_099, 1_000, 100));
    assert!(due(5, 5, 0));
}

#[test]
fn change_filter_len_and_emptiness() {
    let mut f = ChangeFilter::new();
    assert!(f.is_empty());
    assert_eq!(f.len(), 0);
    f.changed("a", "1");
    assert!(!f.is_empty() && f.len() == 1);
}
