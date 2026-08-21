//! Bound decode pool keeps independent reads concurrent; verified via /health occupancy and wall-clock.

mod support;

use std::sync::{Arc, Barrier};
use std::time::Instant;

use support::{Server, dev_dataset, data_copy, skip};

/// Enough callers that the decode pool is genuinely contended.
const CALLERS: usize = 8;

#[test]
fn independent_reads_overlap_inside_a_bounded_decode_pool() {
    // hostile_planes needs 64 chunk decodes to assemble one ortho plane, so the reads are
    // long enough to overlap; two permits make the bound observable at the same time
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::with_args(&["--no-proxy", "--decode-permits", "2"]);
    server.open_project(&dataset);

    let permits = server.json("/health")["reads"]["permits"]
        .as_u64()
        .expect("reads.permits");
    assert_eq!(permits, 2, "the pool was asked for two permits");

    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut threads = Vec::new();
    for caller in 0..CALLERS {
        let client = server.client();
        let token = server.token.clone();
        let url = server.url(&format!(
            "/slice?axis=xz&t={}&cs=0&pos={}",
            caller % 2,
            caller * 8
        ));
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let response = client
                .get(url)
                .bearer_auth(token)
                .send()
                .expect("concurrent slice");
            assert!(response.status().is_success(), "caller {caller}");
            response.bytes().expect("body").len()
        }));
    }
    for thread in threads {
        assert_eq!(
            thread.join().expect("caller thread"),
            64 * 64 * 2,
            "every caller got a whole plane"
        );
    }

    let reads = server.json("/health")["reads"].clone();
    let peak = reads["peak"].as_u64().expect("reads.peak");
    assert!(
        peak >= 2,
        "{CALLERS} simultaneous slices must overlap in the decode pool, not serialize: {reads}"
    );
    assert!(
        peak <= permits,
        "the decode pool must stay bounded by its permits: {reads}"
    );
    assert_eq!(
        reads["inflight"].as_u64().expect("reads.inflight"),
        0,
        "every read released its permit: {reads}"
    );
}

/// The spec's own scenario, timed: two slices that arrive together finish in well under the
/// time they take one after the other.
#[test]
fn dev_dataset_two_slices_finish_faster_together_than_in_sequence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::without_proxy();
    server.open_project(&dataset);

    // every timepoint is a separate chunk, so no read below is served from a warm brick
    let slice = |t: u64| format!("/slice?axis=xz&t={t}&cs=0,1,2&pos=512");
    let time_one = |t: u64| {
        let started = Instant::now();
        let response = server.get(&slice(t)).send().expect("slice");
        assert!(response.status().is_success(), "t={t}");
        let bytes = response.bytes().expect("body").len();
        assert_eq!(bytes, 3 * 3 * 1024 * 2);
        started.elapsed()
    };

    let sequential = time_one(20) + time_one(21);

    let barrier = Arc::new(Barrier::new(2));
    let started = Instant::now();
    let threads: Vec<_> = [22_u64, 23]
        .into_iter()
        .map(|t| {
            let client = server.client();
            let token = server.token.clone();
            let url = server.url(&slice(t));
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let response = client.get(url).bearer_auth(token).send().expect("slice");
                assert!(response.status().is_success(), "t={t}");
                response.bytes().expect("body").len()
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("caller thread"), 3 * 3 * 1024 * 2);
    }
    let together = started.elapsed();

    assert!(
        together.as_secs_f64() < sequential.as_secs_f64() * 0.8,
        "two slices took {together:?} together against {sequential:?} in sequence; \
         they are serializing"
    );
    assert!(
        server.json("/health")["reads"]["peak"]
            .as_u64()
            .expect("reads.peak")
            >= 2,
        "the decode pool never held two reads at once"
    );
}
