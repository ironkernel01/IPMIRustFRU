"""Command requirements from the permitted standards texts."""
from run_conformance import NotTestable, check, le16, test, unavailable


@test("CMD-DEVICE-ID", "Table 10.1.1-1; IPMI Table 20-2", "172; IPMI 271-273",
      "Get Device ID fixed fields, optional auxiliary bytes, support flags and encodings")
def test_device_id(c):
    """VITA Table 10.1.1-1 p.172; IPMI Table 20-2 pp.271-273."""
    d = c.ok(6, 1)
    rule = "IPMI Table 20-2 pp.271-273; Rules 6.2-3 p.123, 7-4 p.134"
    check(len(d) in (11, 15), rule, f"Device ID length {len(d)}, data={d.hex(' ')}")
    check(d[1] & 0x80 != 0, rule, "Device SDR support bit is clear")
    check(d[1] & 0x70 == 0, rule, f"device revision reserved bits {d[1]:02x}")
    check(d[3] >> 4 <= 9 and d[3] & 15 <= 9, rule, f"minor revision is not BCD: {d[3]:02x}")
    check(d[4] in (0x51, 0x02), rule, f"baseline IPMI version {d[4]:02x}, expected 1.5 or 2.0")
    check(d[5] & 9 == 9, rule, f"FRU and sensor support bits missing: {d[5]:02x}")
    check(d[5] & 0x30 == 0, rule, f"Tier-1 advertises event generator/receiver: {d[5]:02x}")
    check(d[8] & 0xF0 == 0, rule, f"identifier reserved bits {d[8]:02x}")
    check(d[6:9] != b"\xff\xff\x0f", rule, "reserved enterprise identifier")
    check(d[9:11] != b"\xff\xff", rule, "reserved device family identifier")


@test("CMD-SELF-TEST", "Table 10.1.1-1; IPMI Table 20-5", "172; IPMI 275",
      "Get Self Test Results byte layout, including permitted unavailable diagnostics")
def test_self_test(c):
    """VITA Table 10.1.1-1 p.172; IPMI Table 20-5 p.275."""
    d = c.ok(6, 4)
    check(len(d) == 2, "IPMI Table 20-5 p.275", f"self-test length/data {d.hex(' ')}")
    check(d[0] != 0xFF, "IPMI Table 20-5 p.275", "reserved self-test result FFh")
    if d[0] in (0x55, 0x56):
        check(d[1] == 0, "IPMI Table 20-5 p.275", f"unused diagnostic byte {d[1]:02x}")


@test("CMD-CAPABILITIES", "Table 10.1.3.1-1; Rules 4.1.4.5.1-4/-5", "46; 183-184",
      "VSO identifier, Layer 1 Tier 1, interface/frequency encoding, standard/revision and FRU IDs")
def test_capabilities(c):
    """VITA §10.1.3.1 pp.183-184; Rules 4.1.4.5.1-4/-5 p.46."""
    d = c.capabilities()
    check(d[0] == 3, "§10.1.2 p.177", f"group ID {d[0]:02x}")
    check(d[1] == 0, "Table 10.1.3.1-1 p.183", f"Layer/Tier/reserved identifier {d[1]:02x}")
    check(d[2] & 0xCC == 0 and d[2] & 3 in (0, 1) and (d[2] >> 4) & 3 in (0, 1),
          "Table 10.1.3.1-1 p.183", f"invalid interface/frequency capabilities {d[2]:02x}")
    check(d[3] == 0, "Table 10.1.3.1-1 p.183", f"standard/reserved byte {d[3]:02x}")
    check(d[4] == 0x22, "§10.1.3.1 p.184", f"2022 approval revision expected 22h, got {d[4]:02x}")
    check(d[5:] == b"\x00\x00", "Rules 4.1.4.5.1-4/-5 p.46", f"Tier-1 FRU IDs {d[5:].hex(' ')}")


@test("CMD-ADDRESS", "§10.1.3.3; Rule 4.1.4.2-2", "41; 186-187",
      "Get FRU Address Info absent/explicit FRU ID, address relation and optional trailing fields")
def test_address(c):
    """VITA §10.1.3.3 pp.186-187; Rule 4.1.4.2-2 p.41."""
    for request in (b"\x03", b"\x03\x00"):
        d = c.ok(0x2C, 0x40, request)
        rule = "§10.1.3.3 pp.186-187"
        check(len(d) in (7, 8, 9), rule, f"address response {d.hex(' ')}")
        check(d[1] < 0x80 and d[2] == c.target.own_address and 2 * d[1] == d[2],
              "Rule 4.1.4.2-2 p.41", f"hardware/IPMB addresses {d[1:3].hex(' ')}")
        check(d[3] == 0xFF and d[4] == 0, rule, f"reserved/FRU ID {d[3:5].hex(' ')}")
        if len(d) >= 8:
            check(d[7] == 0xFF, rule, f"optional reserved byte {d[7]:02x}")
        if d[6] == 0:
            check(d[5] in (0, d[1] - 0x40), "Rules 4.1.4.3-3/-5 p.43",
                  f"front-loading site number {d[5]} for hardware address {d[1]:02x}")
        # Channel-7 addresses and non-front-loading site values are not fixed by
        # the supplied extract. Do not invent a site-type enumeration.


@test("CMD-ADDRESS-PHYSICAL-KEY", "§10.1.3.3", "186-187",
      "Physical-address lookup using the controller's own returned site/type")
def test_address_key(c):
    """VITA §10.1.3.3 pp.186-187."""
    own = c.ok(0x2C, 0x40, b"\x03\x00")
    check(len(own) >= 7, "§10.1.3.3 pp.186-187", "address response lacks site")
    if own[5] == 0:
        raise NotTestable("Controller reports site number unknown; cannot form a known physical address key")
    d = c.ok(0x2C, 0x40, b"\x03\x00\x03" + own[5:7])
    check(d[:7] == own[:7], "§10.1.3.3 pp.186-187", f"own-site lookup differs: {d.hex(' ')}")


@test("CMD-LOCATOR-ID", "Rule 6.2-4; §10.1.3.11", "123; 194",
      "Get Device Locator Record ID exact response and MC locator cross-reference")
def test_locator_id(c):
    """VITA Rule 6.2-4 p.123; §10.1.3.11 p.194."""
    from run_conformance import u16
    d = c.ok(0x2C, 0x0D, b"\x03\x00")
    check(len(d) == 3, "§10.1.3.11 p.194", f"locator-ID response {d.hex(' ')}")
    matches = [r for r in c.records() if u16(r[:2]) == u16(d[1:])]
    check(len(matches) == 1 and matches[0][3] == 0x12, "Rule 6.2-7 p.123",
          f"FRU 0 locator {u16(d[1:]):04x} is not a returned type-12h record")


# Each generated function is a separate requirement/test/report row with its
# own cited docstring. Wrong-length requests retain otherwise valid fields.
LAYOUTS = (
    ("DEVICE-ID", 6, 1, b"", 0, 0, "IPMI Table 20-2", "271"),
    ("SELF-TEST", 6, 4, b"", 0, 0, "IPMI Table 20-5", "275"),
    ("SDR-INFO", 4, 0x20, b"", 0, 1, "IPMI Table 35-2", "481"),
    ("SDR-GET", 4, 0x21, b"\x00\x00\x00\x00\x00\x05", 6, 6, "IPMI Table 35-3", "483"),
    ("SDR-RESERVE", 4, 0x22, b"", 0, 0, "IPMI Table 35-4", "483"),
    ("SENSOR-READ", 4, 0x2D, b"\x00", 1, 1, "IPMI Table 35-15", "497"),
    ("FRU-INFO", 0x0A, 0x10, b"\x00", 1, 1, "IPMI Table 34-2", "476"),
    ("FRU-READ", 0x0A, 0x11, b"\x00\x00\x00\x01", 4, 4, "IPMI Table 34-3", "477"),
    ("FRU-WRITE", 0x0A, 0x12, b"\x00\x00\x00\x01", 4, None, "IPMI Table 34-4", "478"),
    ("VSO-CAP", 0x2C, 0, b"\x03", 1, 1, "VITA §10.1.3.1", "183"),
    ("IPMB-STATE", 0x2C, 9, b"\x03\xff\xff", 3, 4, "VITA §10.1.3.7", "189-190"),
    ("LOCATOR-ID", 0x2C, 0x0D, b"\x03\x00", 2, 2, "VITA §10.1.3.11", "194"),
    ("ADDRESS", 0x2C, 0x40, b"\x03", 1, 5, "VITA §10.1.3.3", "186-187"),
)


def register_length_case(name, nf, cmd, valid, minimum, maximum, rule, page):
    def body(c):
        requests = []
        for length in range(minimum):
            requests.append((valid[:length], (0xC6, 0xC7, 0xFF)))
        if maximum is not None:
            full = valid.ljust(maximum, b"\x00")
            requests.append((full + b"\x00", (0xC7, 0xC8, 0xFF)))
        for data, codes in requests:
            response = c.request(nf, cmd, data)
            c.expect(response, codes, f"{rule} p.{page}; IPMI §5.3 pp.71-72")
    body.__name__ = "test_length_" + name.lower().replace("-", "_")
    body.__doc__ = f"{rule} p.{page}; IPMI Table 5-2 p.70 and §5.3 pp.71-72. C6/C7/C8/FF per error condition."
    test("LEN-" + name, rule + "; IPMI §5.3", page + "; IPMI 71-72",
         "Reject invalid request lengths; accept generic unspecified-error alternative")(body)


for layout in LAYOUTS:
    register_length_case(*layout)


# NA is distinct from O. Optional support is never a Tier-1 failure.
NA_COMMANDS = [(4, n, "Table 10.1.1-1", "173-174")
               for n in (0, 1, 2, *range(0x10, 0x18), 0x28, 0x29, 0x2A, 0x2B)]
NA_COMMANDS += [(0x2C, n, "Table 10.1.2-1", "177-180")
                for n in (1, 2, 3, 0x1B, 0x1C, 0x1D, 0x1F, 0x20, 0x21)]


def register_na(nf, cmd, table, page):
    def body(c):
        data = b"\x03" if nf == 0x2C else b""
        # The command itself is forbidden; request parsing must not implement it.
        c.expect(c.request(nf, cmd, data), (0xC1,), "Rules 4.2-2 p.88, 4.1.1-16 p.31")
    body.__name__ = f"test_na_{nf:02x}_{cmd:02x}"
    body.__doc__ = f"VITA {table} pp.{page}; Rules 4.2-2 p.88, 4.1.1-16 p.31."
    test(f"NA-{nf:02X}-{cmd:02X}", table + "; Rules 4.2-2/4.1.1-16", page + "; 88; 31",
         f"Forbidden Tier-1 command NetFn {nf:02X}h command {cmd:02X}h returns C1h")(body)


for entry in NA_COMMANDS:
    register_na(*entry)


def register_fru_id(name, nf, cmd, suffix):
    def body(c):
        original = c.writable_snapshot() if cmd == 0x12 and nf == 0x0A else None
        try:
            for fru in (1, 2, 127, 254):
                if nf == 0x2C:
                    request = bytes((3, fru)) + suffix
                else:
                    request = bytes((fru,)) + suffix
                    if cmd == 0x12:
                        request = bytes((fru, 0, 0)) + original[:2]
                c.expect(c.request(nf, cmd, request), (0xC9,),
                         "Rules 4.2-3 p.88, 4.1.1-17 p.32")
        finally:
            if original is not None:
                c.restore_fru(original)
    body.__name__ = "test_fru_id_" + name.lower().replace("-", "_")
    body.__doc__ = "VITA Rules 4.2-3 p.88, 4.1.1-17 p.32; FRU IDs 1..254 exceed Tier-1 maximum."
    test("FRUID-" + name, "Rules 4.2-3/4.1.1-17", "88; 32",
         "Nonzero FRU IDs rejected with C9h using otherwise complete requests")(body)


for args in (("AREA-INFO", 0x0A, 0x10, b""),
             ("READ", 0x0A, 0x11, b"\x00\x00\x01"),
             ("WRITE", 0x0A, 0x12, b""),
             ("ADDRESS", 0x2C, 0x40, b""),
             ("LOCATOR", 0x2C, 0x0D, b"")):
    register_fru_id(*args)


@test("CMD-UNSUPPORTED-LUN", "IPMI §5.2, Table 5-2, §5.3", "IPMI 68-72",
      "Probe sensor LUNs and require an error for advertised unimplemented LUNs, echoing RsLUN")
def test_unsupported_lun(c):
    """IPMI §5.2 pp.68-69, Table 5-2 p.70, §5.3 pp.71-72."""
    info = c.ok(4, 0x20)
    check(len(info) >= 2, "IPMI Table 35-2 p.481", "SDR info lacks LUN flags")
    absent = [lun for lun in (1, 2, 3) if not info[1] & (1 << lun)]
    if not absent:
        raise NotTestable("All sensor LUNs are advertised; no known unsupported sensor LUN")
    for lun in absent:
        # A LUN without sensors may still have a command handler. Get Sensor
        # Reading cannot succeed for sensor zero when this LUN has no sensors.
        c.expect(c.request(4, 0x2D, b"\x00", rs_lun=lun), (0xC2, 0xCB, 0xFF),
                 "IPMI §5.2 pp.68-69; §5.3 pp.71-72")


@test("CMD-ADDRESS-INVALID-KEY", "§10.1.3.3; IPMI §5.3", "186-187; IPMI 71-72",
      "Reject a complete address request with unsupported address-key type")
def test_bad_address_key(c):
    """VITA §10.1.3.3 pp.186-187; IPMI §5.3 pp.71-72."""
    c.expect(c.request(0x2C, 0x40, b"\x03\x00\x02\x00\x00"), (0xCC, 0xFF))


@test("CMD-ADDRESS-INCOMPLETE-KEY", "§10.1.3.3; IPMI §5.3", "186-187; IPMI 71-72",
      "Reject physical address-key requests lacking the key or site type")
def test_incomplete_address_key(c):
    """VITA §10.1.3.3 pp.186-187; IPMI §5.3 pp.71-72."""
    for data in (b"\x03\x00\x03", b"\x03\x00\x03\x01"):
        c.expect(c.request(0x2C, 0x40, data), (0xC6, 0xC7, 0xFF))


@test("CMD-GROUP-ID", "§10.1.2; IPMI Table 5-1, §5.3.1", "177; IPMI 68; 71",
      "Reserved defining-body identifier does not dispatch a successful VSO command")
def test_group_identifier(c):
    """VITA §10.1.2 p.177; IPMI Table 5-1 p.68; §5.3.1 p.71."""
    c.expect(c.request(0x2C, 0, b"\xfe"), (0xC1, 0xCC, 0xFF))


@test("CMD-OPTIONAL-POLICY", "Rule 4.1.5.3-4; §10.1.3.8", "51; 191",
      "Optional policy getter: C1h accepted, otherwise exact layout and reserved bits")
def test_optional_policy(c):
    """VITA Rule 4.1.5.3-4 p.51; §10.1.3.8 p.191."""
    d = c.optional(0x0B)
    check(len(d) == 2 and d[1] & 0xF0 == 0, "§10.1.3.8 p.191", f"policy response {d.hex(' ')}")
    for fru in (1, 254):
        c.expect(c.request(0x2C, 0x0B, bytes((3, fru))), (0xC9,), "Rule 4.2-3 p.88")


@test("CMD-OPTIONAL-CONTROL-CAP", "Rule 4.1.2-2/-3; §10.1.3.18", "34; 201",
      "Optional FRU Control Capabilities, cold-reset and power-control dependency probes")
def test_optional_control(c):
    """VITA Rules 4.1.2-2/-3 p.34; §10.1.3.18 p.201."""
    response = c.request(0x2C, 0x1E, b"\x03\x00")
    # Reserved activation/control parameters exercise dispatch without resetting
    # or powering down a payload. Only C1 proves the command absent.
    probes = [c.request(0x2C, cmd, b"\x03\x00\xff") for cmd in (0x04, 0x0C)]
    if all(r.cc == 0xC1 for r in probes) and response.cc == 0xC1:
        raise NotTestable("Optional activation, control and capabilities commands are unsupported (C1h)")
    if any(r.cc != 0xC1 for r in probes):
        c.expect(response, (0,), "Rule 4.1.2-3 p.34")
    d = c.expect(response)
    check(len(d) == 2 and not d[1] & 0xE0, "§10.1.3.18 p.201", f"control capability bytes {d.hex(' ')}")
    if probes[0].cc != 0xC1:
        check(d[1] & 1, "Rule 4.1.2-2 p.34", "FRU Control supported without advertised cold reset")
        c.expect(probes[0], (0xCC,), "Rule 4.1.2-11 p.35")
    if probes[1].cc != 0xC1:
        c.expect(probes[1], (0xCC, 0xFF), "§10.1.3.10 p.193; IPMI §5.3 p.71")
        check(d[1] & 0x10, "§10.1.3.18 p.201", "activation implemented without power-control capability")
        policy = c.request(0x2C, 0x0B, b"\x03\x00")
        c.expect(policy, (0,), "Rule 4.1.5.3-4 p.51")
        c.expect(c.request(0x2C, 0x0A, b"\x03\x00\x00\x00"), (0,), "Rule 4.1.5.3-4 p.51")


# Requirements which cannot be established using this bus-only interface.
# Each remains a separate traceable requirement and executable NT result.
for entry in (
    ("HW-BASELINE", "Rule 2.1-1", "21", "Mechanical, pin-map and electrical baseline compliance",
     "Target observes IPMI frames, not mechanical dimensions, connector wiring or electrical characteristics"),
    ("HW-UNIQUE-ADDRESS", "Rule 4.1.4.1.1-1", "37", "Hardware address is unique within a chassis",
     "Target exposes one controller and no inventory of physical controllers; own address relation is checked separately"),
    ("HW-STRAPS", "Rules 4.1.4.1.1-2/-3", "38", "Odd number of address strap pins plus parity pin",
     "Target has no pin inspection or strap controls"),
    ("HW-ODD-PARITY", "Rule 4.1.4.1.1-4", "38", "Hardware address strap parity is odd",
     "Target exposes no address/parity pin levels"),
    ("HW-PARTIAL-ADDRESS", "Rule 4.1.4.1.1-5", "38", "Assign the unstrapped portion of the hardware address",
     "Cannot identify or vary partially hardwired address pins through Target"),
    ("HW-FRONT-ADDRESS", "Rule 4.1.4.1.2-1", "39", "Front-module hardware address derives from GA pins plus 40h",
     "Target has no GA strap or module-form-factor input; returned address/site consistency is tested separately"),
    ("HW-PARITY-PIN", "Rule 4.1.4.1.2-2", "39-40", "Use designated parity pin for front module",
     "Requires physical pin wiring inspection"),
    ("HW-INVALID-PARITY-ADDRESS", "Rule 4.1.4.2-4", "41", "No IPMB address assigned on invalid hardware parity",
     "Target cannot change parity/address straps"),
    ("HW-INVALID-PARITY-ISOLATE", "Rule 4.1.5.4.1-5", "53-54", "Tier-1 isolates from IPMB on invalid hardware parity",
     "Target cannot change parity straps or observe electrical isolation"),
    ("HW-INITIAL-CRITERIA", "Rules 4.1.5.4.1-1/-2/-3/-4/-8/-9", "53-54", "M0-to-M1 initial checks of power, address, SDR and diagnostics",
     "Cannot control management power, straps, diagnostic faults or local SDR corruption through Target"),
    ("HW-PAYLOAD-INTERFACE", "Rule 4.1.3-1", "36", "If a Payload Interface is implemented, it accepts IPMI at LUN 0",
     "Target transports reach System IPMB only, not a local Payload Interface"),
    ("HW-SYSRESET", "Rule 4.1.9-1", "69", "Controller reset behavior does not depend on external reset signal",
     "Target has no external reset signal control"),
    ("HW-RESET-ISOLATION", "Rule 4.1.9-2", "69", "All backplane IPMB connections isolate electrically while reset is held",
     "Target.reset is instantaneous from this interface and cannot hold reset or measure electrical isolation"),
    ("HW-PINS", "Table 9.2.1.2-1; Table 9.1.2-1", "161; 157", "System IPMB connector pin assignments",
     "Requires connector wiring inspection"),
    ("HW-REAR-PINS", "Rule 9.2.1.3-1", "162", "Rear modules do not connect reserved System IPMB pins",
     "Target cannot identify module placement or inspect connector wiring"),
    ("HW-CAPACITANCE", "Rule 9.2.1.2-2", "161", "Front-module IPMB capacitance at most 22 pF",
     "Requires capacitance measurement unavailable through Target"),
    ("HW-ELECTRICAL", "Rules 9.2.2-1/-2; Table 9.2.2-1", "163-165", "IPMB clock, voltage, rise-time and electrical limits",
     "No electrical waveform capture, bus-rate setting or calibrated voltage measurement in Target"),
    ("HW-NO-PULLUPS", "Rule 9.2.3-2", "166-167", "IPMC does not implement System IPMB pull-ups",
     "Requires physical circuit inspection or electrical measurement"),
    ("HW-SEPARATE-BUFFERS", "Rule 9.3-1", "167-168", "Each supported IPMB has a separate buffer",
     "Bus isolation behavior cannot establish internal circuit topology"),
    ("HW-WATCHDOG-ISOLATE", "Rule 9.3-3", "168", "If present, watchdog expiry disables IPMB buffers",
     "Target cannot force controller watchdog expiry or measure buffer enable signals"),
    ("LINK-CONTROLLED-CHANGES", "Rule 9.3-10", "170", "Local IPMB state changes only on command or persistent fault transitions",
     "Unbounded temporal behavior and all persistent electrical faults cannot be established through finite frame capture; no-change commands are checked"),
    ("OPTIONAL-MASTER-WRITE-READ", "Rule 4.1.1-15", "31", "Optional Master Write-Read cannot reach System IPMB-attached devices",
     "Target lacks a known benign System IPMB peripheral and transaction capture; executing arbitrary peripheral writes would not independently establish routing"),
    ("OPTIONAL-SEND-MESSAGE", "Rules 4.1.1-12/-13/-20..24", "30-33", "Conditional bridging and firewall completion behavior if Send Message exists",
     "Extract omits full conditional rule texts; Target has no controlled bridge destination or privilege/session interface"),
    ("OPTIONAL-WARM-RESET", "Rule 4.1.9-4", "70", "Optional Warm Reset preserves payload state and FRU Operational State",
     "Target cannot independently observe payload state or hold state-machine criteria stable across reset"),
    ("OPTIONAL-STATE-POLICY-INIT", "Rule 4.1.5.4.1-11", "55", "Conditional Activation-Locked initialization on M0-to-M1",
     "Cannot control initial payload power criteria or observe the exact M0-to-M1 transition"),
    ("OPTIONAL-STATE-POLICY-ATOMIC", "Rules 4.1.5.3-2/-3", "51", "Unsupported policy update changes no bits; supported changes precede reply",
     "Policy bits may be fixed; Target cannot identify a safely mutable bit while preserving power/state behavior"),
    ("OPTIONAL-STATE-M1", "Rules 4.1.5.4.2-1/-3/-4", "55-56", "Conditional M1 payload activation criteria and policy behavior",
     "No payload-power measurement/control or stable state/criteria fixture in Target"),
    ("OPTIONAL-STATE-M4", "Rules 4.1.5.4.3-1..-7", "56-58", "Conditional M4 deactivation criteria, commands and CDI restrictions",
     "No payload-power or deactivation-criteria fixture in Target; cannot establish all conditional states"),
    ("OPTIONAL-STATE-M5", "Rules 4.1.5.4.4-1..-5", "58-59", "Conditional M5 activation/deactivation handling",
     "No controllable deactivation-request criteria or payload-power observation"),
    ("OPTIONAL-STATE-M6", "Rules 4.1.5.4.5-1/-2/-3", "59-60", "Conditional M6 transition and activate/deactivate completion codes",
     "No payload shutdown/hold fixture; cannot reproducibly enter and hold M6"),
    ("OPTIONAL-FRU-CONTROL-RESET", "Rules 4.1.2-2/-9", "34-35", "Implemented FRU Control provides hardware-equivalent cold reset",
     "Capabilities and reserved-option errors are checked; Target cannot measure payload hardware reset equivalence"),
    ("OPTIONAL-HIGHER-TIER-FEATURES", "§§4.1.11-4.1.17; Table 10.1.2-1", "72-88; 177-180", "Conditional fan, indicator, power, mode, persistent control and firewall behavior",
     "Permitted extract omits most optional request/response tables and hardware prerequisites; missing optional support is a compliant Tier-1 choice"),
):
    unavailable(*entry)


@test("CMD-ADDRESS-RANGES", "Rules 4.1.4.1.1-6/-7; Tables 4.1.4.1.1-1/-2", "38-39",
      "Assigned hardware address avoids reserved values and belongs to a defined address class")
def test_address_ranges(c):
    """VITA Rules 4.1.4.1.1-6/-7 and Tables 4.1.4.1.1-1/-2 pp.38-39."""
    d = c.ok(0x2C, 0x40, b"\x03")
    check(len(d) >= 7, "§10.1.3.3 pp.186-187", "address response lacks hardware address/site")
    hw = d[1]
    check(hw not in (0, 0x77) and not 0x28 <= hw <= 0x2F,
          "Rule 4.1.4.1.1-6 p.38", f"reserved hardware address {hw:02x}")
    if d[6] == 0:
        check(0x41 <= hw <= 0x5F, "Table 4.1.4.1.1-2 p.39", f"front-module hardware address {hw:02x}")
