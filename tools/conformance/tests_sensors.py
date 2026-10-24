"""Mandatory sensors, polled layouts, and SDR cross-checks."""
from run_conformance import NotTestable, check, test, u16, unavailable


SENSORS = (
    (0, "STATE", 0xF0, 0x6F, "10.2.1-1", "238"),
    (1, "LINK", 0xF1, 0x6F, "10.2.2-1", "241-242"),
    (2, "HEALTH", 0xF2, 0x04, "10.2.3-1", "243"),
    (3, "VOLTAGE", 0x02, 0x05, "10.2.4-1", "245"),
    (4, "TEMPERATURE", 0xF3, 0x6F, "10.2.5-1", "246-247"),
    (5, "TEST-RESULTS", 0xF4, 0x04, "10.2.6-1", "248"),
    (6, "TEST-STATUS", 0xF5, 0x03, "10.2.7-1", "249"),
)


def sensor_case(number, name, sensor_type, event_type, table, page):
    def body(c):
        key = (c.target.own_address, 0, number)
        records = c.sensor_records()
        rule = "Rule 6.2-1 p.123; Table 10.2-1 p.236"
        check(key in records, rule, f"mandatory sensor {number} absent from LUN 0 SDRs")
        r = records[key]
        check(r[3] in (1, 2) and len(r) >= 20, rule,
              f"mandatory polled sensor {number} needs full/compact readable SDR: {r.hex(' ')}")
        check((r[12], r[13]) == (sensor_type, event_type), rule,
              f"sensor {number} type/event {r[12]:02x}/{r[13]:02x}, expected {sensor_type:02x}/{event_type:02x}")
        d = c.reading(number)
        specific = f"Table {table} p.{page}"
        if number == 0:
            check(d[0] == 0, specific, f"FRU State sensor reading must be 00h: {d.hex(' ')}")
        if number in (0, 1) and len(d) == 4:
            check(d[3] == 0x80, specific, f"optional high-state byte must be 80h: {d.hex(' ')}")
        if d[1] & 0x20:
            # Valid 'reading unavailable' is part of the format, especially for
            # payload tests that have never run. Its state bits are not evidence.
            c.trace.append(f"sensor {number}: unavailable flag set; state semantics not evaluated")
            return
        state, mask = d[2], u16(r[18:20]) & 0x7FFF
        check(state & ~mask == 0, "IPMI Tables 43-1/43-2 pp.555/561",
              f"sensor {number} state {state:02x} not in SDR reading mask {mask:04x}")
        if number == 0:
            check(state in (1, 2, 0x10, 0x20, 0x40), specific,
                  f"FRU state is not one of M0/M1/M4/M5/M6: {state:02x}")
        elif number == 1:
            check(state & 0xC0 == 0, specific, f"reserved link state bits {state:02x}")
            check(state & 15 in (1, 2, 4, 8), specific, f"link enable states not one-hot: {state:02x}")
        elif number in (2, 3, 5, 6):
            check(state in (1, 2), specific, f"sensor {number} needs one of its two states: {state:02x}")
        elif number == 4:
            check(state & 0xC0 == 0, specific, f"temperature state has undefined bits: {state:02x}")
    body.__name__ = "test_sensor_" + name.lower().replace("-", "_")
    body.__doc__ = f"VITA Rule 6.2-1 p.123; Table 10.2-1 p.236; Table {table} pp.{page}."
    test("SENSOR-" + name, "Rule 6.2-1; Table 10.2-1; Table " + table,
         "123; 236; " + page, f"Sensor {number} on FRU 0 LUN 0: type, event code, reading layout and SDR mask")(body)


for sensor in SENSORS:
    sensor_case(*sensor)


@test("SENSOR-UNASSIGNED", "IPMI Table 35-15; Table 5-2, §5.3", "IPMI 497; 70-72",
      "Unknown sensor number yields CBh or the allowed generic unspecified error")
def test_unknown_sensor(c):
    """IPMI Table 35-15 p.497; Table 5-2 p.70; §5.3 pp.71-72."""
    occupied = {n for owner, lun, n in c.sensor_records() if owner == c.target.own_address and lun == 0}
    number = next((n for n in range(254, 6, -1) if n not in occupied), None)
    if number is None:
        raise NotTestable("Every usable sensor number on LUN 0 has an SDR")
    c.expect(c.request(4, 0x2D, bytes((number,))), (0xCB, 0xFF))


@test("SENSOR-MANDATORY-NUMBERS", "Rule 6.2-2; §10.1.3.26; Rule 4.1.5.4.1-10", "123; 207-208; 55",
      "If implemented, Get Mandatory Sensor Numbers agrees with fixed FRU 0 SDR numbers and M0 error")
def test_mandatory_numbers(c):
    """VITA Rule 6.2-2 p.123; §10.1.3.26 pp.207-208; Rule 4.1.5.4.1-10 p.55."""
    state = c.reading(0)
    response = c.request(0x2C, 0x44, b"\x03\x00")
    if response.cc == 0xC1:
        raise NotTestable("Table 10.1.2-1 pp.178-179 marks this optional; Rule 6.2-2 p.123 conflicts (review required)")
    if not state[1] & 0x20 and state[2] == 1:
        c.expect(response, (0xD3,), "Rule 4.1.5.4.1-10 p.55")
        return
    d = c.expect(response)
    check(7 <= len(d) <= 9, "§10.1.3.26 pp.207-208", f"mandatory numbers response {d.hex(' ')}")
    check(d[:7] == b"\x03\x00\x02\x03\x04\x05\x06", "Table 10.2-1 p.236",
          f"FRU 0 mandatory sensor numbers {d.hex(' ')}")
    if len(d) >= 8 and d[7] != 0xFF:
        key = (c.target.own_address, 0, d[7])
        records = c.sensor_records()
        check(key in records and records[key][12] == 0xF6, "§10.1.3.26 p.208; Table 10.2-3 p.237",
              "external-standard sensor number does not name a matching sensor SDR")
    if len(d) == 9 and d[8] != 0xFF:
        key = (c.target.own_address, 0, d[8])
        records = c.sensor_records()
        check(key in records and records[key][12:14] == b"\xf7\x6f", "Table 10.2-1 p.236",
              "optional payload mode number does not name a matching sensor SDR")
    for fru in (1, 254):
        c.expect(c.request(0x2C, 0x44, bytes((3, fru))), (0xC9,), "Rule 4.2-3 p.88")


@test("SENSOR-TYPE-RESERVATIONS", "Rule 6.2-12; Table 10.2-3", "124; 237",
      "No sensor uses F9h..FFh reserved sensor types")
def test_type_reservations(c):
    """VITA Rule 6.2-12 p.124; Table 10.2-3 p.237."""
    for key, r in c.sensor_records().items():
        if r[3] in (1, 2):
            check(len(r) >= 14, "IPMI Tables 43-1/43-2 pp.551/558", f"short sensor record {key}")
            check(r[12] < 0xF9, "Rule 6.2-12 p.124; Table 10.2-3 p.237",
                  f"sensor {key} uses reserved type {r[12]:02x}")


unavailable("SENSOR-TEMPERATURE-STIMULUS", "§4.1.6; §4.1.7", "62-64",
            "Temperature sensor reflects real threshold crossings under polling",
            "Target has no temperature, voltage or payload-test stimulus controls; layouts and SDRs are tested separately")
