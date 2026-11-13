// link_vectors.rs - replays the captured IPMB traffic (local_only/vectors/sbc_ab.vec) through the link layer
// with IPMB-B fitted and enabled, the way the bench build cannot (one Aardvark, B not fitted).

mod common;
use common::load;
use ipmc_core::addr;
use ipmc_core::dispatch::Context;
use ipmc_core::ext::NoExtension;
use ipmc_core::ipmb;
use ipmc_core::link::{BusMode, Link, LinkConfig, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::trace::NoTrace;

/// The capture's controller answers at IPMB 82h, the address of GA 1 (addr::resolve(0x7F)).
const OWN: u8 = 0x82;

/// One Context per replay: handler state (FRU writes, SDR reservations) carries across frames
/// the way it does on the device.
fn ctx() -> Context {
    Context::new(addr::resolve(0x7F))
}

fn link(policy: TxPolicy) -> Link {
    let mut link = Link::new(LinkConfig::bench(policy, false), &addr::resolve(0x7F));
    link.set_ipmb_state(None, Some(BusMode::LocalControl))
        .expect("enable B");
    link
}

/// What the capture holds, counted from the file.
struct Expected {
    requests: [u32; 2],
    responses: [u32; 2],
    not_for_us: [u32; 2],
}

fn expected() -> Expected {
    let mut e = Expected {
        requests: [0; 2],
        responses: [0; 2],
        not_for_us: [0; 2],
    };
    for v in load() {
        let frame = ipmb::parse(&v.bytes).expect("parse");
        let i = v.bus as usize;
        if frame.header.is_response() {
            e.responses[i] += 1;
        } else if frame.header.dest == OWN {
            e.requests[i] += 1;
        } else {
            e.not_for_us[i] += 1;
        }
    }
    e
}

/// Feeds every captured frame at its capture time, polls, and sends whatever is queued with
/// success. Returns the (bus, request seq) of every transmitted response, in order.
fn replay(link: &mut Link) -> Vec<(Bus, u8)> {
    let mut sent = Vec::new();
    let mut ctx = ctx();
    for v in load() {
        let bus = v.bus;
        let _ = link.on_frame(bus, &v.bytes, v.time_ms);
        link.poll(&mut ctx, &mut NoExtension, v.time_ms, &mut NoTrace);
        while let Some((tx_bus, frame)) = link.next_tx() {
            let response = ipmb::parse(frame).expect("response parses");
            sent.push((tx_bus, response.header.seq));
            link.on_tx_result(tx_bus, true, v.time_ms, &mut NoTrace)
                .expect("tx result");
        }
    }
    sent
}

fn assert_counts(link: &Link, e: &Expected) {
    let total_requests: u32 = e.requests.iter().sum();
    for (i, bus) in [Bus::A, Bus::B].into_iter().enumerate() {
        let c = link.counters(bus);
        assert_eq!(c.rx_ok, e.requests[i], "rx_ok {}", bus.name());
        assert_eq!(c.rx_response, e.responses[i], "rx_response {}", bus.name());
        assert_eq!(c.not_for_us, e.not_for_us[i], "not_for_us {}", bus.name());
        assert_eq!(c.rx_errors.total(), 0, "parse errors {}", bus.name());
        assert_eq!(c.ingress_full, 0);
        assert_eq!(c.replays, 0, "false duplicate on {}", bus.name());
        assert_eq!((c.tx_fail, c.resends, c.tx_dropped), (0, 0, 0));
    }
    assert_eq!(
        link.counters(Bus::A).tx_ok + link.counters(Bus::B).tx_ok,
        total_requests,
        "every request answered once"
    );
    // The capture polls at up to 16 requests per second; with IPMB_DUP_CACHE entries kept for
    // IPMB_DUP_WINDOW_MS the cache holds every answer of the window (no eviction, PLAN.md 3.3),
    // and a false duplicate (the live Chassis Manager reuses rqSeq after about 3.85 s) fails the
    // replays assert above.
    assert_eq!(link.evictions(), 0, "duplicate cache evictions");
    assert_eq!(link.dispatch_errors(), 0);
    assert_eq!(link.egress_dropped(), 0);
    assert_eq!(link.late_responses(), 0);
}

#[test]
fn alternate_policy_sends_every_response_once_alternating_buses() {
    if common::capture_missing() {
        return;
    }
    let e = expected();
    let mut l = link(TxPolicy::Alternate);
    let sent = replay(&mut l);
    assert_counts(&l, &e);
    assert_eq!(sent.len() as u32, e.requests.iter().sum::<u32>());
    for (i, (bus, _)) in sent.iter().enumerate() {
        let expected_bus = if i % 2 == 0 { Bus::A } else { Bus::B };
        assert_eq!(*bus, expected_bus, "response {i}");
    }
    let tx = [l.counters(Bus::A).tx_ok, l.counters(Bus::B).tx_ok];
    assert!(tx[0].abs_diff(tx[1]) <= 1, "alternation is even: {tx:?}");
}

#[test]
fn arrival_policy_answers_on_the_request_bus() {
    if common::capture_missing() {
        return;
    }
    let e = expected();
    let mut l = link(TxPolicy::Arrival);
    let sent = replay(&mut l);
    assert_counts(&l, &e);
    // Requests in file order, with the bus each arrived on.
    let arrival: Vec<(Bus, u8)> = load()
        .iter()
        .filter_map(|v| {
            let f = ipmb::parse(&v.bytes).ok()?;
            (!f.header.is_response() && f.header.dest == OWN).then_some((v.bus, f.header.seq))
        })
        .collect();
    assert_eq!(sent, arrival);
    // Same counts per bus as the capture.
    assert_eq!(l.counters(Bus::A).tx_ok, e.requests[0]);
    assert_eq!(l.counters(Bus::B).tx_ok, e.requests[1]);
}

/// The Chassis Manager of the capture alternates buses and retries on the other bus when a
/// response does not arrive. A request on A whose response transmit fails is resent on B and
/// the later retry of the same request, arriving on B, is answered from the cache.
#[test]
fn failed_response_on_a_is_resent_on_b_and_the_retry_is_replayed() {
    if common::capture_missing() {
        return;
    }
    let first_on_a = load()
        .into_iter()
        .find(|v| v.bus == Bus::A && ipmb::parse(&v.bytes).is_ok_and(|f| !f.header.is_response()))
        .expect("a request on A");
    let mut l = link(TxPolicy::Arrival);
    let mut ctx = ctx();
    l.on_frame(Bus::A, &first_on_a.bytes, first_on_a.time_ms)
        .expect("accepted");
    l.poll(&mut ctx, &mut NoExtension, first_on_a.time_ms, &mut NoTrace);
    let (bus, original) = l.next_tx().map(|(b, f)| (b, f.to_vec())).expect("response");
    assert_eq!(bus, Bus::A);
    l.on_tx_result(Bus::A, false, first_on_a.time_ms, &mut NoTrace)
        .expect("result");
    let (bus, resent) = l.next_tx().map(|(b, f)| (b, f.to_vec())).expect("resend");
    assert_eq!(bus, Bus::B);
    assert_eq!(resent, original);
    l.on_tx_result(Bus::B, true, first_on_a.time_ms, &mut NoTrace)
        .expect("result");
    assert_eq!(
        (l.counters(Bus::A).tx_fail, l.counters(Bus::B).resends),
        (1, 1)
    );

    // The requester timed out meanwhile and retries the same request on B.
    l.on_frame(Bus::B, &first_on_a.bytes, first_on_a.time_ms + 250)
        .expect("accepted");
    l.poll(
        &mut ctx,
        &mut NoExtension,
        first_on_a.time_ms + 250,
        &mut NoTrace,
    );
    let (_, replayed) = l.next_tx().map(|(b, f)| (b, f.to_vec())).expect("replay");
    assert_eq!(replayed, original);
    assert_eq!(l.counters(Bus::B).replays, 1);
}
