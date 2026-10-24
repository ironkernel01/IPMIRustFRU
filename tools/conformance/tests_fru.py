"""FRU inventory checks and checksum-preserving, reversible write probes."""
from run_conformance import NotTestable, check, le16, test, unavailable


def common(c):
    image = c.fru_image()
    check(len(image) >= 8, "Rule 8.1-2 p.138", "common header absent")
    return image, image[:8]


def area(c, index):
    image, header = common(c)
    offset = header[index] * 8
    check(offset >= 8 and offset + 2 <= len(image), "Rule 8.1-2 p.138", f"area {index} offset {offset}")
    size = image[offset + 1] * 8
    check(size >= 8 and offset + size <= len(image), "FRU §§11-12 pp.7-8",
          f"area {index} range {offset}+{size} outside image {len(image)}")
    return offset, image[offset:offset + size]


@test("FRU-AREA-INFO", "Rules 8.1-1/-5; IPMI Table 34-2", "138-139; IPMI 476",
      "FRU 0 on LUN 0: exact inventory info layout and byte/word access encoding")
def test_area_info(c):
    """VITA Rules 8.1-1/-5 pp.138-139; IPMI Table 34-2 p.476."""
    size, unit = c.fru_info()
    check(unit in (1, 2) and size >= 8, "IPMI Table 34-2 p.476", f"size={size}, access unit={unit}")


@test("FRU-READ", "Rule 8.1-5; IPMI Table 34-3", "139; IPMI 477",
      "Read entire inventory in bounded chunks; validate returned counts and byte-exact overlapping reads")
def test_read(c):
    """VITA Rule 8.1-5 p.139; IPMI Table 34-3 p.477."""
    image = c.fru_image()
    _, unit = c.fru_info()
    for offset, count in ((0, 1), (0, 8), (unit, 3), (len(image) - 1, 1)):
        if offset + count <= len(image):
            data = c.fru_read(offset, count)
            check(data == image[offset:offset + count], "IPMI Table 34-3 p.477",
                  f"offset/count {offset}/{count} differs: {data.hex(' ')}")


@test("FRU-READ-OFFSET", "IPMI Table 34-3; Table 5-2, §5.3", "IPMI 477; 70-72",
      "Read starting past the FRU size rejected with out-of-range or permitted unspecified error")
def test_read_offset(c):
    """IPMI Table 34-3 p.477; Table 5-2 p.70; §5.3 pp.71-72."""
    size, unit = c.fru_info()
    offset = (size + unit - 1) // unit
    c.expect(c.request(0x0A, 0x11, b"\x00" + le16(offset) + b"\x01"), (0xC9, 0xFF))


@test("FRU-READ-COUNT", "IPMI Table 34-3; Table 5-2, §5.3", "IPMI 477; 70-72",
      "Reject zero read count; oversized reads return bounded data or a permitted transfer-size error")
def test_read_count(c):
    """IPMI Table 34-3 p.477; Table 5-2 p.70; §5.3 pp.71-72."""
    c.expect(c.request(0x0A, 0x11, b"\x00\x00\x00\x00"), (0xC9, 0xCC, 0xFF))
    response = c.request(0x0A, 0x11, b"\x00\x00\x00\xff")
    c.expect(response, (0, 0xC8, 0xC9, 0xCA, 0xFF))
    if response.cc == 0:
        data = response.data
        check(len(data) >= 2 and data[0] == len(data) - 1, "IPMI Table 34-3 p.477",
              f"large-read count mismatch: {data.hex(' ')}")
        check(data[1:] == c.fru_read(0, data[0]), "IPMI Table 34-3 p.477", "large read bytes differ")


@test("FRU-COMMON-HEADER", "Rule 8.1-2; FRU Table 8-1", "138; FRU 5",
      "Common header version, offsets, mandatory Board/Product areas, pad and checksum")
def test_header(c):
    """VITA Rule 8.1-2 p.138; FRU Table 8-1 p.5 and §17 p.15."""
    image, h = common(c)
    rule = "FRU Table 8-1 p.5"
    check(h[0] == 1, rule, f"header format/reserved bits {h[0]:02x}")
    check(h[6] == 0, rule, f"header padding {h[6]:02x}")
    check(sum(h) & 255 == 0, rule, f"common header checksum {h.hex(' ')}")
    check(h[3] != 0 and h[4] != 0, "Rule 8.1-2 p.138", "mandatory Board/Product area absent")
    offsets = [v * 8 for v in h[1:6] if v]
    check(all(8 <= v < len(image) for v in offsets), rule, f"area offsets outside image: {offsets}")
    check(offsets == sorted(set(offsets)), "FRU §17 p.15", f"overlapping/out-of-order area starts: {offsets}")
    if h[1]:
        check(image[h[1] * 8] == 1, "FRU Table 9-1 p.5", "Internal Use Area version is not 1")


@test("FRU-NO-CHASSIS-AREA", "Rule 8.1-3", "139",
      "Board FRU common header has zero Chassis Info Area offset")
def test_no_chassis(c):
    """VITA Rule 8.1-3 p.139."""
    _, h = common(c)
    check(h[2] == 0, "Rule 8.1-3 p.139", f"chassis area offset {h[2]:02x}")


def area_case(index, name, start, mandatory_fields, section, page):
    def body(c):
        offset, d = area(c, index)
        image, h = common(c)
        rule = f"FRU {section} p.{page}"
        check(d[0] == 1, rule, f"{name} format/reserved bits {d[0]:02x}")
        check(sum(d) & 255 == 0, rule, f"{name} checksum at {offset}: {d.hex(' ')}")
        next_offsets = [v * 8 for v in h[index + 1:6] if v]
        check(not next_offsets or offset + len(d) <= min(next_offsets), "FRU §17 p.15",
              f"{name} overlaps following area")
        pos, fields = start, 0
        while pos < len(d) - 1 and d[pos] != 0xC1:
            length = d[pos] & 0x3F
            check(pos + 1 + length < len(d), rule, f"{name} field at {pos} crosses checksum")
            fields += 1
            pos += 1 + length
        check(fields >= mandatory_fields, "FRU §5 p.3",
              f"{name} has {fields} fields; {mandatory_fields} predefined fields required even when empty")
        check(pos < len(d) - 1 and d[pos] == 0xC1, rule, f"{name} missing end-of-fields marker")
        check(all(value == 0 for value in d[pos + 1:-1]), rule, f"{name} unused padding is not zero")
    body.__name__ = "test_" + name.lower() + "_area"
    body.__doc__ = f"VITA Rule 8.1-2 p.138; FRU {section} p.{page}, §5 p.3."
    test("FRU-" + name + "-AREA", "Rule 8.1-2; FRU " + section, "138; FRU " + page,
         name + " area version, size, checksum, required type/length fields, terminator and padding")(body)


area_case(3, "BOARD", 6, 5, "Table 11-1", "7")
area_case(4, "PRODUCT", 3, 7, "Table 12-1", "8")


@test("FRU-MULTIRECORD", "FRU §§16-17; VITA Table 10.3-1", "FRU 13-15; 255",
      "If present, validate MultiRecord header/data checksums, length and end-of-list")
def test_multirecord(c):
    """FRU §§16-17 pp.13-15; VITA Table 10.3-1 p.255."""
    image, h = common(c)
    if not h[5]:
        raise NotTestable("Optional MultiRecord area is absent; permitted by Permission 8.1-4 p.139")
    pos = h[5] * 8
    while pos < len(image):
        header = image[pos:pos + 5]
        check(len(header) == 5, "FRU Table 16-1 p.13", f"truncated record header at {pos}")
        check(sum(header) & 255 == 0, "FRU §16.2.6 p.14", f"record header checksum at {pos}")
        check(header[1] & 0x70 == 0 and header[1] & 15 == 2,
              "FRU §16.2.3 p.14", f"record flags/version {header[1]:02x}")
        data = image[pos + 5:pos + 5 + header[2]]
        check(len(data) == header[2], "FRU §16.2.4 p.14", f"record extends past image at {pos}")
        check((sum(data) + header[3]) & 255 == 0, "FRU §16.2.5 p.14", f"record data checksum at {pos}")
        if header[0] == 0xC0 and data[:3] == b"\xac\x81\x00":
            check(len(data) >= 5 and data[4] == 0, "VITA Table 10.3-1 p.255",
                  f"VITA record format revision at {pos}: {data.hex(' ')}")
        pos += 5 + len(data)
        if header[1] & 0x80:
            return
    check(False, "FRU §16.2.2 p.14", "MultiRecord list lacks end-of-list marker")


@test("FRU-WRITE", "Rule 8.1-5; IPMI Table 34-4; Rules 8-4/-5", "139; IPMI 478; 137",
      "Write original header bytes, validate count or 80h protection, restore original bytes and verify full image")
def test_write(c):
    """VITA Rule 8.1-5 p.139; Rules 8-4/-5 p.137; IPMI Table 34-4 p.478."""
    original = c.writable_snapshot()
    check(sum(original[:8]) & 255 == 0, "Rule 8-2 p.137", "preexisting header checksum invalid; write probe withheld")
    try:
        r = c.request(0x0A, 0x12, b"\x00\x00\x00" + original[:8])
        if r.cc == 0x80:
            check(c.fru_read(0, 8) == original[:8], "Rule 8-5 p.137", "protected header changed")
            c.trace.append("Write FRU Data is implemented; header is protected (80h). Writable behavior not established.")
            return
        d = c.expect(r)
        check(len(d) == 1 and 1 <= d[0] <= 8, "IPMI Table 34-4 p.478", f"count written {d.hex(' ')}")
        check(c.fru_read(0, 8) == original[:8], "Rule 8.1-5 p.139", "write/readback differs")
    finally:
        # An explicit write-back is performed even though the probe used original
        # bytes. Protected storage legitimately rejects both writes with 80h.
        try:
            r = c.request(0x0A, 0x12, b"\x00\x00\x00" + original[:8])
            c.expect(r, (0, 0x80), "Rule 8.1-5 p.139; Rule 8-5 p.137")
        finally:
            c.restore_fru(original)


@test("FRU-WRITE-OFFSET", "IPMI Table 34-4; Table 5-2, §5.3", "IPMI 478; 70-72",
      "Reject out-of-range write; original full image restored and verified even after failure")
def test_write_offset(c):
    """IPMI Table 34-4 p.478; Table 5-2 p.70; §5.3 pp.71-72; VITA Rule 8-5 p.137."""
    original = c.writable_snapshot()
    size, unit = c.fru_info()
    try:
        request = b"\x00" + le16((size + unit - 1) // unit) + original[:unit]
        c.expect(c.request(0x0A, 0x12, request), (0xC9, 0x80, 0xFF),
                 "IPMI §5.3 p.71; VITA Rule 8-5 p.137 (protection can take precedence)")
    finally:
        c.restore_fru(original)


unavailable("FRU-POWER-INDEPENDENCE", "Rule 8-1", "136",
            "FRU inventory remains available with payload activated and deactivated",
            "Target cannot control or independently measure payload power; FRU reads cover only current power state")
unavailable("FRU-CHECKSUM-INITIATOR", "Rule 8-2", "137",
            "Controller accepts initiator-computed checksums without imposing FRU semantics",
            "A deliberate invalid-checksum write conflicts with the required checksum-preserving test policy; valid images and writes are covered")
