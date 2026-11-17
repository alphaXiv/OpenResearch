use orx_signals::Snapshot;

#[test]
fn periodic_pause_boundaries_survive_timing_retention() {
    let mut snapshot = Snapshot::new("run", "file", "generation");
    for step in 1..=5000 {
        let t = step + ((step - 1) / 100) * 150;
        let line =
            format!("::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":5000,\"t\":{t}}}\n");
        snapshot
            .feed(line.as_bytes(), false, t * 1000, true)
            .unwrap();
        snapshot.reached_end(t * 1000);
    }
    let pauses: Vec<_> = snapshot
        .timing
        .progress_points
        .windows(2)
        .filter(|pair| pair[1].1 - pair[0].1 > 100.0 && pair[1].2 - pair[0].2 == 1.0)
        .map(|pair| pair[0].2)
        .collect();
    assert!(pauses.len() >= 3, "{pauses:?}");
    assert!(
        pauses.windows(2).any(|pair| pair[1] - pair[0] == 100.0),
        "{pauses:?}"
    );
}
