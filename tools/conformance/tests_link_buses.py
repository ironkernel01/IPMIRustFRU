"""System IPMB state and optional dual-bus behavior."""
from contextlib import contextmanager

from run_conformance import NotTestable, check, test, unavailable


def settings(c):
    d = c.reading(1)
    if d[1] & 0x20:
        raise NotTestable("Link sensor reading unavailable; original bus override/speed settings cannot be saved")
    speed = (1 if d[2] & 0x10 else 0) | (4 if d[2] & 0x20 else 0)
    return (1 if d[0] & 8 else 0, 1 if d[0] & 0x80 else 0, speed)


def set_state(c, a, b, speed=None, *, bus="A"):
    request = bytes((3, a, b)) + (b"" if speed is None else bytes((speed,)))
    response = c.request(0x2C, 9, request, bus=bus)
    if response.cc == 0:
        check(response.data == b"\x03", "§10.1.3.7 pp.189-190", f"Set IPMB State response {response.data.hex(' ')}")
    return response


@contextmanager
def two_buses(c):
    if len(c.target.buses) < 2:
        raise NotTestable(f"Transport {c.target.transport} exposes {c.target.buses}; needs two capturable buses")
    if c.capabilities()[2] & 3 != 1:
        raise NotTestable("Controller advertises only IPMB-A; IPMB-B is optional for Tier-1")
    original = settings(c)
    try:
        c.expect(set_state(c, 1, 1), (0,), "Rules 4.1.1-5/-6 p.28")
        # A valid request on B also establishes that the peer can use that bus.
        c.ok(6, 1, bus="B")
        yield
    finally:
        # Restore a reachable A path first, then saved overrides and speeds.
        c.expect(set_state(c, 1, 1), (0,), "§10.1.3.7 pp.189-190 (restore reachability)")
        a, b, speed = original
        c.expect(set_state(c, a, b, speed), (0,), "§10.1.3.7 pp.189-190 (restore original state)")


@test("LINK-A-ACCESS", "Rules 4.1.1-1/-5; 9.2.1.1-1", "26; 28; 160",
      "Implemented mandatory IPMB-A accepts a valid request")
def test_a(c):
    """VITA Rules 4.1.1-1 p.26, 4.1.1-5 p.28, 9.2.1.1-1 p.160."""
    if "A" not in c.target.buses:
        raise NotTestable("Transport has no IPMB-A drive/capture capability")
    c.ok(6, 1, bus="A")


@test("LINK-STATE-NO-CHANGE", "§10.1.3.7; Rule 9.3-10", "189-190; 170",
      "Set IPMB State FFh no-change leaves override and speed fields unchanged")
def test_no_change(c):
    """VITA §10.1.3.7 pp.189-190; Rule 9.3-10 p.170."""
    original = settings(c)
    c.expect(set_state(c, 0xFF, 0xFF))
    check(settings(c) == original, "§10.1.3.7 pp.189-190", "FFh changed override/speed fields")


@test("LINK-REFUSE-ISOLATION", "Rule 4.1.8-3", "68",
      "Refuse isolation of both interfaces and remain reachable without changing overrides")
def test_refuse_isolation(c):
    """VITA Rule 4.1.8-3 p.68; Rule 4.1.1-14 p.31."""
    original = settings(c)
    response = set_state(c, 0, 0)
    check(response.cc != 0, "Rule 4.1.8-3 p.68", "accepted isolation of all IPMB interfaces")
    check(settings(c) == original, "Rule 4.1.8-3 p.68", "refused request changed original link settings")


@test("LINK-UNSUPPORTED-ATOMIC", "Rule 4.1.1-14; §10.1.3.7", "31; 189-190",
      "Unsupported link/speed parameters return CCh and make no change")
def test_unsupported(c):
    """VITA Rule 4.1.1-14 p.31; §10.1.3.7 pp.189-190."""
    original = settings(c)
    # Link ID 60h and frequency code 10b are explicitly reserved. Use a valid
    # no-change value on the other interface to avoid a second error condition.
    for a, b, speed in ((0xC1, 0xFF, None), (0xFF, 0xFF, 0x02)):
        c.expect(set_state(c, a, b, speed), (0xCC,), "Rule 4.1.1-14 p.31")
        check(settings(c) == original, "Rule 4.1.1-14 p.31", "unsupported request partially changed state")


@test("LINK-OPTIONAL-SPEED", "Table 3.3-1; §10.1.3.7", "24-25; 189-190",
      "2022 controller accepts the optional speed byte at current supported speed")
def test_speed_byte(c):
    """VITA Table 3.3-1 pp.24-25; §10.1.3.7 pp.189-190; Obs.4.1.1-2.3 p.28."""
    original = settings(c)
    c.expect(set_state(c, 0xFF, 0xFF, original[2]), (0,), "Table 3.3-1 pp.24-25")
    check(settings(c) == original, "§10.1.3.7 pp.189-190", "setting current speed changed settings")


@test("LINK-BOOT", "Rules 4.1.1-4/-4.3; 4.1.8-1", "27-28; 68",
      "Following reset, A is enabled, B disabled, and speed is STANDARD")
def test_boot(c):
    """VITA Rules 4.1.1-4 p.27, 4.1.1-4.3 p.28, 4.1.8-1 p.68."""
    if c.target.transport != "emu":
        raise NotTestable("Transport does not provide an observable controlled reset/power-up")
    try:
        c.target.reset()
    except NotImplementedError:
        raise NotTestable("Target.reset is unavailable on this transport") from None
    c.cache.clear()
    r = c.request(4, 0x2D, b"\x01")
    d = c.expect(r)
    check(r.bus == "A", "Rule 4.1.8-1 p.68", f"initial transmission used {r.bus}")
    check(len(d) in (3, 4) and not d[1] & 0x20, "Table 10.2.2-1 pp.241-242", f"boot link reading {d.hex(' ')}")
    check(d[2] & 0x3F == 2, "Rules 4.1.1-4/-4.3 pp.27-28", f"boot enable/speed state {d[2]:02x}")


@test("LINK-B-ACCESS", "Rule 4.1.1-5", "28",
      "When B is implemented/enabled, both A and B accept requests equally")
def test_b_access(c):
    """VITA Rule 4.1.1-5 p.28."""
    with two_buses(c):
        a = c.ok(6, 1, bus="A")
        b = c.ok(6, 1, bus="B")
        check(a == b, "Rule 4.1.1-5 p.28", f"Device ID differs by receiving bus: {a.hex(' ')} / {b.hex(' ')}")


@test("LINK-ALTERNATION", "Rule 4.1.1-6", "28",
      "With two enabled healthy interfaces, successive IPMC transmissions alternate buses")
def test_alternation(c):
    """VITA Rule 4.1.1-6 p.28. No request/response bus-affinity assumption."""
    with two_buses(c):
        observed = []
        for _ in range(8):
            response = c.request(6, 1, bus="A")
            c.expect(response)
            observed.append(response.bus)
        check(all(a != b for a, b in zip(observed, observed[1:])), "Rule 4.1.1-6 p.28",
              f"successive transmission buses {observed}")


@test("LINK-ISOLATION-LOCAL", "§10.1.3.7; Rules 9.3-2/-4", "189-190; 168-169",
      "Isolate A while B stays local; sensor reports overrides/enable state, restore A then isolate B")
def test_isolation_local(c):
    """VITA §10.1.3.7 pp.189-190; Rules 9.3-2/-4 pp.168-169; Table 10.2.2-1 pp.241-242."""
    with two_buses(c):
        try:
            c.expect(set_state(c, 0, 1, bus="B"))
            d = c.reading(1, bus="B")
            check(not d[0] & 8 and d[0] & 0x80 and d[2] & 15 == 4,
                  "Table 10.2.2-1 pp.241-242", f"A isolated/B local state {d.hex(' ')}")
            seq = c.seq()
            frame = c.adapter.frame(c.target.own_address, 6, 0, c.target.requester_address, seq, 0, 1, b"")
            c.raw_reply(frame, 6, 1, seq, bus="A", expect_reply=False)
        finally:
            # A is isolated: restoration must be addressed over reachable B.
            c.expect(set_state(c, 1, 1, bus="B"))
        c.expect(set_state(c, 1, 0))
        d = c.reading(1)
        check(d[0] & 8 and not d[0] & 0x80 and d[2] & 15 == 2,
              "Table 10.2.2-1 pp.241-242", f"A local/B isolated state {d.hex(' ')}")
        seq = c.seq()
        frame = c.adapter.frame(c.target.own_address, 6, 0, c.target.requester_address, seq, 0, 1, b"")
        c.raw_reply(frame, 6, 1, seq, bus="B", expect_reply=False)


@test("LINK-FAILOVER", "Rule 4.1.1-7", "28",
      "When the next transmit bus fails, receive the response over the enabled alternate bus")
def test_failover(c):
    """VITA Rule 4.1.1-7 p.28; failure injection requires two buses and can_fail_bus."""
    if not c.target.can_fail_bus:
        raise NotTestable(f"Transport {c.target.transport} cannot fail a bus")
    with two_buses(c):
        for failed in ("A", "B"):
            # Align the next alternating transmission with the bus to fail.
            for _ in range(3):
                r = c.request(6, 1)
                c.expect(r)
                if r.bus != failed:
                    break
            healthy = "B" if failed == "A" else "A"
            try:
                c.target.bus_down(failed)
                first_timing = len(c.timings)
                try:
                    response = c.request(6, 1, bus=healthy)
                except AssertionError as error:
                    if "no response within 250 ms" not in str(error):
                        raise
                    # Loss does not prove that the IPMC observed an unsuccessful
                    # transmit (the contract has no acknowledgement control).
                    del c.timings[first_timing:]
                    raise NotTestable("Injected bus loss produced no captured reply; Target does not establish detectable transmit failure, so alternate resend cannot be judged") from error
                c.expect(response)
                check(response.bus == healthy, "Rule 4.1.1-7 p.28",
                      f"failed={failed}, received={response.bus}")
            finally:
                c.target.bus_up(failed)
            c.expect(set_state(c, 1, 1))


@test("LINK-INFO-OPTIONAL", "Rules 4.1.8-4/-5; §10.1.3.17", "68; 200",
      "If Get IPMB Link Info exists, both mandatory keys map link 0 to sensor 1; others C9h")
def test_link_info(c):
    """VITA Rules 4.1.8-4/-5 p.68; §10.1.3.17 p.200."""
    for key in (b"\x03\x00\x00", b"\x03\x01\x01"):
        d = c.optional(0x18, key)
        check(d == b"\x03\x00\x01", "Rule 4.1.8-4 p.68", f"link mapping {d.hex(' ')}")
    for key in (b"\x03\x00\x01", b"\x03\x01\x00", b"\x03\x02\x00"):
        c.expect(c.request(0x2C, 0x18, key), (0xC9,), "Rule 4.1.8-5 p.68")


unavailable("LINK-FAILOVER-ATTEMPT", "Rule 4.1.1-7", "28",
            "Observe the failed physical transmit attempt before alternate-interface resend",
            "Target reports only received frames; bus_down loses frames and exposes neither failed transmit attempts nor acknowledgement status")
unavailable("LINK-UNKNOWN-PEER-B", "Rule 4.1.8-2", "68",
            "Until peer IPMB-B support is known, originate requests only on A",
            "Tier-1 generates no events; Target cannot cause arbitrary outbound requests or configure peer capability knowledge")
unavailable("LINK-PERSISTENT-FAULT", "§9.3", "167-169",
            "Local buffer fault diagnosis, isolation and recovery for persistent electrical faults",
            "bus_down drops frames; it does not drive clock/data electrical fault levels or expose buffer diagnostic controls; extract gives no diagnostic timeout")
