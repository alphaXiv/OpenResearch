use orx_signals::Snapshot;

#[test]
fn periodic_pause_boundaries_survive_timing_retention() {
    let mut snapshot = Snapshot::new("run", "file", "generation");
    let mut t = 0;
    for step in 1..=5000 {
        t += 1;
        if step > 1 && (step - 1) % 100 == 0 {
            t += if [3, 7, 19, 41].contains(&(step / 100)) {
                300
            } else {
                150
            };
        }
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
    assert!(pauses.starts_with(&[100.0, 200.0, 300.0]), "{pauses:?}");
}
