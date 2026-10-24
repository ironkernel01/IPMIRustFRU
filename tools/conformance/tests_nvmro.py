"""Protection behavior observable without an NVMRO hardware control."""
from run_conformance import NotTestable, check, le16, test, unavailable


@test("PROTECTION-OFFSETS", "Rules 8-4/-5; IPMI Table 34-4", "137; IPMI 478",
      "Probe original bytes across Common/Board/Product areas; protected writes use 80h with no data change")
def test_protected_offsets(c):
    """VITA Rules 8-4/-5 p.137; IPMI Table 34-4 p.478."""
    from tests_fru import area, common
    original = c.writable_snapshot()
    _, unit = c.fru_info()
    spans = [(0, 8)]
    for index in (3, 4):
        offset, content = area(c, index)
        spans.append((offset, len(content)))
    any_protected = False
    try:
        for offset, size in spans:
            codes = set()
            # Every byte/word in each applicable area, rather than endpoints
            # only, so a partial-area protection map is observable.
            for position in range(offset, offset + size, unit):
                saved = original[position:position + unit]
                r = c.request(0x0A, 0x12, b"\x00" + le16(position // unit) + saved)
                c.expect(r, (0, 0x80), "Rule 8-5 p.137")
                codes.add(r.cc)
                if r.cc == 0:
                    check(len(r.data) == 1 and 0 < r.data[0] <= len(saved),
                          "IPMI Table 34-4 p.478", f"write count at {position}: {r.data.hex(' ')}")
            check(len(codes) == 1, "Rule 8-4 p.137", f"protection varies within area {offset}+{size}: {codes}")
            any_protected |= 0x80 in codes
    finally:
        c.restore_fru(original)
    if not any_protected:
        raise NotTestable("No protected byte in current Common/Board/Product areas; 80h behavior cannot be induced through Target")


unavailable("NVMRO-ASSERTED", "Rule 8-3; §4.1.10", "137; 70-71",
            "When NVMRO is asserted, all FRU information is write-protected",
            "Target exposes neither NVMRO state nor assertion/deassertion control; a protected FRU alone does not establish NVMRO causation")
unavailable("NVMRO-OTHER-NVM", "Recs.4.1.10-1/-2", "70-71",
            "Recommended NVMRO protection of other controller nonvolatile memory",
            "Recommendations are not mandatory Tier-1 failures; Target has no NVMRO control or generic nonvolatile-memory access")
unavailable("PROTECTION-COMMAND-DEPENDENCY", "Rule 4.1.17-1; §§10.1.3.34-36", "88; 217-220",
            "If Set Write Protect Enables is implemented, both protection getter commands are implemented",
            "Permitted extract gives command IDs and dependency but omits request/response layouts; cannot issue a valid setter or interpret the capability masks independently")
unavailable("PROTECTION-INTERNAL-MULTIRECORD", "Rules 8-4/-5", "137",
            "Protection granularity of optional Internal Use area and individual MultiRecord records",
            "Target has no way to configure protection maps; optional internal data must not be altered speculatively. Common/Board/Product current-map probes are separate")
