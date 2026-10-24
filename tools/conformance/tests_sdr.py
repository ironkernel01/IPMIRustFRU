"""Device SDR tests; repository records are discovered through the bus."""
from run_conformance import NotTestable, check, le16, test, u16, unavailable


@test("SDR-INFO", "Rules 6.2-3/-5; IPMI Table 35-2", "123; IPMI 481",
      "Get Device SDR Info default, sensor count, SDR count, LUN and population fields")
def test_info(c):
    """VITA Rules 6.2-3/-5 p.123; IPMI Table 35-2 p.481."""
    default = c.ok(4, 0x20)
    for operation in (b"", b"\x00", b"\x01"):
        d = c.ok(4, 0x20, operation)
        check(len(d) in (2, 6), "IPMI Table 35-2 p.481", f"SDR info bytes {d.hex(' ')}")
        check(len(d) == (6 if d[1] & 0x80 else 2), "IPMI Table 35-2 p.481",
              f"dynamic flag/timestamp layout {d.hex(' ')}")
        check(d[1] & 1, "Rule 6.2-1 p.123; Table 10.2-1 p.236", "LUN 0 sensors not advertised")
        if operation != b"\x01" and not default[1] & 0x80:
            check(d == default, "IPMI Table 35-2 p.481", "static default/explicit sensor info differs")
        count = len(c.records()) if operation == b"\x01" else sum(
            owner == c.target.own_address and lun == 0 for owner, lun, number in c.sensor_records())
        if not d[1] & 0x80:
            check(d[0] == count, "Rules 6.2-8/-11 p.124; IPMI Table 35-2 p.481",
                  f"reported count {d[0]}, discovered {count}")


@test("SDR-RESERVE", "Rule 6.2-5; IPMI Table 35-4", "123; IPMI 483",
      "Reserve Device SDR Repository returns a nonzero little-endian two-byte ID")
def test_reservation(c):
    """VITA Rule 6.2-5 p.123; IPMI Table 35-4 p.483."""
    c.reserve()


@test("SDR-CHAIN", "Rules 6.2-3/-8; IPMI §33.12, Table 35-3", "123-124; IPMI 471; 483",
      "Partial-read all record bytes; validate lengths, actual record IDs, noncyclic next chain to FFFFh")
def test_chain(c):
    """VITA Rules 6.2-3/-8 pp.123-124; IPMI §33.12 p.471, Table 35-3 p.483."""
    records = c.records()
    check(bool(records), "Rule 6.2-7 p.123", "empty Device SDR repository")
    for r in records:
        check(len(r) == 5 + r[4], "IPMI §43 p.551", f"record {u16(r[:2]):04x} length {len(r)} != 5+{r[4]}")
        check(r[2] == 0x51, "IPMI §43 p.551", f"SDR version {r[2]:02x}")
        # Other standard/OEM record types are allowed. Mandatory sensor records
        # and the MC locator have their own type-specific validation below.


@test("SDR-PARTIAL-CONSISTENCY", "IPMI §35.3, Table 35-3", "IPMI 482-483",
      "Compare overlapping reads and reservation-free offset-zero reads against reconstructed records")
def test_partial(c):
    """IPMI §35.3 pp.482-483 and Table 35-3 p.483."""
    for r in c.records():
        rid = u16(r[:2])
        reservation = c.reserve()
        for offset in (0, 1, len(r) - 1):
            if offset > 255:
                continue
            count = min(8, len(r) - offset)
            _, part = c.sdr_part(reservation if offset else 0, rid, offset, count)
            check(part == r[offset:offset + count], "IPMI Table 35-3 p.483",
                  f"record={rid:04x} offset={offset} differs: {part.hex(' ')}")


@test("SDR-INVALID-RESERVATION", "IPMI §33.11.1, Table 35-3", "IPMI 470; 483",
      "Nonzero-offset partial read rejects zero/invalid reservation")
def test_invalid_reservation(c):
    """IPMI §33.11.1 p.470; Table 35-3 p.483; §5.3 pp.71-72."""
    rid = u16(c.records()[0][:2])
    c.reserve()
    c.expect(c.request(4, 0x21, b"\x00\x00" + le16(rid) + b"\x01\x01"),
             (0xC5,), "IPMI §33.11.1 p.470; §5.3 p.71")


@test("SDR-CANCEL-RESERVATION", "IPMI §33.11.2, §35.4", "IPMI 470; 483",
      "A new reservation cancels the preceding reservation for partial reads")
def test_cancel(c):
    """IPMI §33.11.2 p.470; §35.4 p.483."""
    rid = u16(c.records()[0][:2])
    old, new = c.reserve(), c.reserve()
    check(old != new, "IPMI §33.11.2 p.470", "new reservation did not invalidate old token")
    c.expect(c.request(4, 0x21, le16(old) + le16(rid) + b"\x01\x01"),
             (0xC5,), "IPMI §33.11.2 p.470; §5.3 p.71")
    c.sdr_part(new, rid, 1, 1)


@test("SDR-BAD-ID", "IPMI Table 35-3; Table 5-2, §5.3", "IPMI 483; 70-72",
      "Missing actual SDR ID rejected; do not confuse 0000h/FFFFh selectors with missing IDs")
def test_bad_id(c):
    """IPMI Table 35-3 p.483; Table 5-2 p.70; §5.3 pp.71-72."""
    ids = {u16(r[:2]) for r in c.records()}
    candidate = next((i for i in range(1, 65535) if i not in ids), None)
    if candidate is None:
        raise NotTestable("All usable SDR record IDs exist")
    c.expect(c.request(4, 0x21, b"\x00\x00" + le16(candidate) + b"\x00\x01"), (0xCB, 0xFF))


@test("SDR-BAD-OFFSET", "IPMI Table 35-3; Table 5-2, §5.3", "IPMI 483; 70-72",
      "Out-of-range SDR offset rejected under a valid reservation")
def test_bad_offset(c):
    """IPMI Table 35-3 p.483; Table 5-2 p.70; §5.3 pp.71-72."""
    candidates = [r for r in c.records() if len(r) <= 255]
    if not candidates:
        raise NotTestable("No record has an out-of-range offset representable in one byte")
    r = candidates[0]
    c.expect(c.request(4, 0x21, le16(c.reserve()) + r[:2] + bytes((len(r), 1))), (0xC9, 0xFF))


@test("SDR-MC-LOCATOR", "Rule 6.2-7; IPMI Table 43-8", "123; IPMI 573-574",
      "MC Device Locator record address, sensor/FRU/device-SDR capability bits and string length")
def test_locator(c):
    """VITA Rule 6.2-7 p.123; IPMI Table 43-8 pp.573-574."""
    locators = [r for r in c.records() if r[3] == 0x12 and len(r) >= 6 and
                r[5] & 0xFE == c.target.own_address]
    check(len(locators) >= 1, "Rule 6.2-7 p.123", "no MC locator for this IPMC/FRU 0")
    for r in locators:
        rule = "IPMI Table 43-8 pp.573-574"
        check(len(r) >= 16, rule, f"short MC locator {r.hex(' ')}")
        check(r[8] & 0x0B == 0x0B, rule, f"missing FRU/device-SDR/sensor capability: {r[8]:02x}")
        check(not r[8] & 0x30, "Rule 7-4 p.134; Table 10.1.1-1 p.173", "MC locator advertises event support")
        length = r[15] & 0x1F
        check(length <= 16 and len(r) == 16 + length, rule, f"MC locator string/record length {r.hex(' ')}")


@test("SDR-SENSOR-RECORDS", "Rules 6.2-8/-11; IPMI Tables 43-1/43-2", "124; IPMI 551-563",
      "Full/compact sensor records: fixed lengths, string bounds, key uniqueness and per-LUN readable sensors")
def test_sensor_records(c):
    """VITA Rules 6.2-8/-11 p.124; IPMI Tables 43-1/43-2 pp.551-563."""
    for key, r in c.sensor_records().items():
        if r[3] not in (1, 2):
            continue
        index = 47 if r[3] == 1 else 31
        rule = "IPMI Tables 43-1/43-2 pp.551-563"
        check(len(r) > index, rule, f"short sensor record {r.hex(' ')}")
        check(len(r) == index + 1 + (r[index] & 0x1F) and r[index] & 0x1F <= 16,
              rule, f"sensor string/record length {r.hex(' ')}")
        if key[0] == c.target.own_address:
            c.expect(c.request(4, 0x2D, bytes((key[2],)), rs_lun=key[1]))


unavailable("SDR-FIRMWARE-POPULATION", "Rule 6.2-13", "124",
            "Firmware-upgradable controllers advertise Dynamic Population = 1",
            "Target exposes no firmware-upgrade capability; cannot establish the conditional prerequisite from permitted texts")
