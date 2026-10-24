# Tier-1 bus conformance results

Transport: `emu`; buses: ('A', 'B'); fault injection: True.

Each request uses retries=0. T5 is checked against adapter STOP-to-STOP timestamps.
Raw send/receive has no STOP timestamps or receive-bus metadata; those limits are listed explicitly.

| ID | Rule/table/section | Page | Result | Detail |
|---|---|---|---|---|
| CMD-DEVICE-ID | Table 10.1.1-1; IPMI Table 20-2 | 172; IPMI 271-273 | PASS | Get Device ID fixed fields, optional auxiliary bytes, support flags and encodings |
| CMD-SELF-TEST | Table 10.1.1-1; IPMI Table 20-5 | 172; IPMI 275 | PASS | Get Self Test Results byte layout, including permitted unavailable diagnostics |
| CMD-CAPABILITIES | Table 10.1.3.1-1; Rules 4.1.4.5.1-4/-5 | 46; 183-184 | PASS | VSO identifier, Layer 1 Tier 1, interface/frequency encoding, standard/revision and FRU IDs |
| CMD-ADDRESS | §10.1.3.3; Rule 4.1.4.2-2 | 41; 186-187 | PASS | Get FRU Address Info absent/explicit FRU ID, address relation and optional trailing fields |
| CMD-ADDRESS-PHYSICAL-KEY | §10.1.3.3 | 186-187 | PASS | Physical-address lookup using the controller's own returned site/type |
| CMD-LOCATOR-ID | Rule 6.2-4; §10.1.3.11 | 123; 194 | PASS | Get Device Locator Record ID exact response and MC locator cross-reference |
| LEN-DEVICE-ID | IPMI Table 20-2; IPMI §5.3 | 271; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SELF-TEST | IPMI Table 20-5; IPMI §5.3 | 275; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-INFO | IPMI Table 35-2; IPMI §5.3 | 481; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-GET | IPMI Table 35-3; IPMI §5.3 | 483; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SDR-RESERVE | IPMI Table 35-4; IPMI §5.3 | 483; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-SENSOR-READ | IPMI Table 35-15; IPMI §5.3 | 497; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-INFO | IPMI Table 34-2; IPMI §5.3 | 476; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-READ | IPMI Table 34-3; IPMI §5.3 | 477; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-FRU-WRITE | IPMI Table 34-4; IPMI §5.3 | 478; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-VSO-CAP | VITA §10.1.3.1; IPMI §5.3 | 183; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-IPMB-STATE | VITA §10.1.3.7; IPMI §5.3 | 189-190; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-LOCATOR-ID | VITA §10.1.3.11; IPMI §5.3 | 194; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| LEN-ADDRESS | VITA §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject invalid request lengths; accept generic unspecified-error alternative |
| NA-04-00 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 00h returns C1h |
| NA-04-01 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 01h returns C1h |
| NA-04-02 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 02h returns C1h |
| NA-04-10 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 10h returns C1h |
| NA-04-11 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 11h returns C1h |
| NA-04-12 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 12h returns C1h |
| NA-04-13 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 13h returns C1h |
| NA-04-14 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 14h returns C1h |
| NA-04-15 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 15h returns C1h |
| NA-04-16 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 16h returns C1h |
| NA-04-17 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 17h returns C1h |
| NA-04-28 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 28h returns C1h |
| NA-04-29 | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 29h returns C1h |
| NA-04-2A | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 2Ah returns C1h |
| NA-04-2B | Table 10.1.1-1; Rules 4.2-2/4.1.1-16 | 173-174; 88; 31 | PASS | Forbidden Tier-1 command NetFn 04h command 2Bh returns C1h |
| NA-2C-01 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 01h returns C1h |
| NA-2C-02 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 02h returns C1h |
| NA-2C-03 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 03h returns C1h |
| NA-2C-1B | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Bh returns C1h |
| NA-2C-1C | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Ch returns C1h |
| NA-2C-1D | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Dh returns C1h |
| NA-2C-1F | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 1Fh returns C1h |
| NA-2C-20 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 20h returns C1h |
| NA-2C-21 | Table 10.1.2-1; Rules 4.2-2/4.1.1-16 | 177-180; 88; 31 | PASS | Forbidden Tier-1 command NetFn 2Ch command 21h returns C1h |
| FRUID-AREA-INFO | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-READ | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-WRITE | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-ADDRESS | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| FRUID-LOCATOR | Rules 4.2-3/4.1.1-17 | 88; 32 | PASS | Nonzero FRU IDs rejected with C9h using otherwise complete requests |
| CMD-UNSUPPORTED-LUN | IPMI §5.2, Table 5-2, §5.3 | IPMI 68-72 | PASS | Probe sensor LUNs and require an error for advertised unimplemented LUNs, echoing RsLUN |
| CMD-ADDRESS-INVALID-KEY | §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject a complete address request with unsupported address-key type |
| CMD-ADDRESS-INCOMPLETE-KEY | §10.1.3.3; IPMI §5.3 | 186-187; IPMI 71-72 | PASS | Reject physical address-key requests lacking the key or site type |
| CMD-GROUP-ID | §10.1.2; IPMI Table 5-1, §5.3.1 | 177; IPMI 68; 71 | PASS | Reserved defining-body identifier does not dispatch a successful VSO command |
| CMD-OPTIONAL-POLICY | Rule 4.1.5.3-4; §10.1.3.8 | 51; 191 | NOT-TESTABLE | Optional command is unsupported (C1h), a compliant Tier-1 choice |
| CMD-OPTIONAL-CONTROL-CAP | Rule 4.1.2-2/-3; §10.1.3.18 | 34; 201 | NOT-TESTABLE | Optional activation, control and capabilities commands are unsupported (C1h) |
| HW-BASELINE | Rule 2.1-1 | 21 | NOT-TESTABLE | Target observes IPMI frames, not mechanical dimensions, connector wiring or electrical characteristics |
| HW-UNIQUE-ADDRESS | Rule 4.1.4.1.1-1 | 37 | NOT-TESTABLE | Target exposes one controller and no inventory of physical controllers; own address relation is checked separately |
| HW-STRAPS | Rules 4.1.4.1.1-2/-3 | 38 | NOT-TESTABLE | Target has no pin inspection or strap controls |
| HW-ODD-PARITY | Rule 4.1.4.1.1-4 | 38 | NOT-TESTABLE | Target exposes no address/parity pin levels |
| HW-PARTIAL-ADDRESS | Rule 4.1.4.1.1-5 | 38 | NOT-TESTABLE | Cannot identify or vary partially hardwired address pins through Target |
| HW-FRONT-ADDRESS | Rule 4.1.4.1.2-1 | 39 | NOT-TESTABLE | Target has no GA strap or module-form-factor input; returned address/site consistency is tested separately |
| HW-PARITY-PIN | Rule 4.1.4.1.2-2 | 39-40 | NOT-TESTABLE | Requires physical pin wiring inspection |
| HW-INVALID-PARITY-ADDRESS | Rule 4.1.4.2-4 | 41 | NOT-TESTABLE | Target cannot change parity/address straps |
| HW-INVALID-PARITY-ISOLATE | Rule 4.1.5.4.1-5 | 53-54 | NOT-TESTABLE | Target cannot change parity straps or observe electrical isolation |
| HW-INITIAL-CRITERIA | Rules 4.1.5.4.1-1/-2/-3/-4/-8/-9 | 53-54 | NOT-TESTABLE | Cannot control management power, straps, diagnostic faults or local SDR corruption through Target |
| HW-PAYLOAD-INTERFACE | Rule 4.1.3-1 | 36 | NOT-TESTABLE | Target transports reach System IPMB only, not a local Payload Interface |
| HW-SYSRESET | Rule 4.1.9-1 | 69 | NOT-TESTABLE | Target has no external reset signal control |
| HW-RESET-ISOLATION | Rule 4.1.9-2 | 69 | NOT-TESTABLE | Target.reset is instantaneous from this interface and cannot hold reset or measure electrical isolation |
| HW-PINS | Table 9.2.1.2-1; Table 9.1.2-1 | 161; 157 | NOT-TESTABLE | Requires connector wiring inspection |
| HW-REAR-PINS | Rule 9.2.1.3-1 | 162 | NOT-TESTABLE | Target cannot identify module placement or inspect connector wiring |
| HW-CAPACITANCE | Rule 9.2.1.2-2 | 161 | NOT-TESTABLE | Requires capacitance measurement unavailable through Target |
| HW-ELECTRICAL | Rules 9.2.2-1/-2; Table 9.2.2-1 | 163-165 | NOT-TESTABLE | No electrical waveform capture, bus-rate setting or calibrated voltage measurement in Target |
| HW-NO-PULLUPS | Rule 9.2.3-2 | 166-167 | NOT-TESTABLE | Requires physical circuit inspection or electrical measurement |
| HW-SEPARATE-BUFFERS | Rule 9.3-1 | 167-168 | NOT-TESTABLE | Bus isolation behavior cannot establish internal circuit topology |
| HW-WATCHDOG-ISOLATE | Rule 9.3-3 | 168 | NOT-TESTABLE | Target cannot force controller watchdog expiry or measure buffer enable signals |
| LINK-CONTROLLED-CHANGES | Rule 9.3-10 | 170 | NOT-TESTABLE | Unbounded temporal behavior and all persistent electrical faults cannot be established through finite frame capture; no-change commands are checked |
| OPTIONAL-MASTER-WRITE-READ | Rule 4.1.1-15 | 31 | NOT-TESTABLE | Target lacks a known benign System IPMB peripheral and transaction capture; executing arbitrary peripheral writes would not independently establish routing |
| OPTIONAL-SEND-MESSAGE | Rules 4.1.1-12/-13/-20..24 | 30-33 | NOT-TESTABLE | Extract omits full conditional rule texts; Target has no controlled bridge destination or privilege/session interface |
| OPTIONAL-WARM-RESET | Rule 4.1.9-4 | 70 | NOT-TESTABLE | Target cannot independently observe payload state or hold state-machine criteria stable across reset |
| OPTIONAL-STATE-POLICY-INIT | Rule 4.1.5.4.1-11 | 55 | NOT-TESTABLE | Cannot control initial payload power criteria or observe the exact M0-to-M1 transition |
| OPTIONAL-STATE-POLICY-ATOMIC | Rules 4.1.5.3-2/-3 | 51 | NOT-TESTABLE | Policy bits may be fixed; Target cannot identify a safely mutable bit while preserving power/state behavior |
| OPTIONAL-STATE-M1 | Rules 4.1.5.4.2-1/-3/-4 | 55-56 | NOT-TESTABLE | No payload-power measurement/control or stable state/criteria fixture in Target |
| OPTIONAL-STATE-M4 | Rules 4.1.5.4.3-1..-7 | 56-58 | NOT-TESTABLE | No payload-power or deactivation-criteria fixture in Target; cannot establish all conditional states |
| OPTIONAL-STATE-M5 | Rules 4.1.5.4.4-1..-5 | 58-59 | NOT-TESTABLE | No controllable deactivation-request criteria or payload-power observation |
| OPTIONAL-STATE-M6 | Rules 4.1.5.4.5-1/-2/-3 | 59-60 | NOT-TESTABLE | No payload shutdown/hold fixture; cannot reproducibly enter and hold M6 |
| OPTIONAL-FRU-CONTROL-RESET | Rules 4.1.2-2/-9 | 34-35 | NOT-TESTABLE | Capabilities and reserved-option errors are checked; Target cannot measure payload hardware reset equivalence |
| OPTIONAL-HIGHER-TIER-FEATURES | §§4.1.11-4.1.17; Table 10.1.2-1 | 72-88; 177-180 | NOT-TESTABLE | Permitted extract omits most optional request/response tables and hardware prerequisites; missing optional support is a compliant Tier-1 choice |
| CMD-ADDRESS-RANGES | Rules 4.1.4.1.1-6/-7; Tables 4.1.4.1.1-1/-2 | 38-39 | PASS | Assigned hardware address avoids reserved values and belongs to a defined address class |
| SDR-INFO | Rules 6.2-3/-5; IPMI Table 35-2 | 123; IPMI 481 | PASS | Get Device SDR Info default, sensor count, SDR count, LUN and population fields |
| SDR-RESERVE | Rule 6.2-5; IPMI Table 35-4 | 123; IPMI 483 | PASS | Reserve Device SDR Repository returns a nonzero little-endian two-byte ID |
| SDR-CHAIN | Rules 6.2-3/-8; IPMI §33.12, Table 35-3 | 123-124; IPMI 471; 483 | PASS | Partial-read all record bytes; validate lengths, actual record IDs, noncyclic next chain to FFFFh |
| SDR-PARTIAL-CONSISTENCY | IPMI §35.3, Table 35-3 | IPMI 482-483 | PASS | Compare overlapping reads and reservation-free offset-zero reads against reconstructed records |
| SDR-INVALID-RESERVATION | IPMI §33.11.1, Table 35-3 | IPMI 470; 483 | PASS | Nonzero-offset partial read rejects zero/invalid reservation |
| SDR-CANCEL-RESERVATION | IPMI §33.11.2, §35.4 | IPMI 470; 483 | PASS | A new reservation cancels the preceding reservation for partial reads |
| SDR-BAD-ID | IPMI Table 35-3; Table 5-2, §5.3 | IPMI 483; 70-72 | PASS | Missing actual SDR ID rejected; do not confuse 0000h/FFFFh selectors with missing IDs |
| SDR-BAD-OFFSET | IPMI Table 35-3; Table 5-2, §5.3 | IPMI 483; 70-72 | PASS | Out-of-range SDR offset rejected under a valid reservation |
| SDR-MC-LOCATOR | Rule 6.2-7; IPMI Table 43-8 | 123; IPMI 573-574 | PASS | MC Device Locator record address, sensor/FRU/device-SDR capability bits and string length |
| SDR-SENSOR-RECORDS | Rules 6.2-8/-11; IPMI Tables 43-1/43-2 | 124; IPMI 551-563 | PASS | Full/compact sensor records: fixed lengths, string bounds, key uniqueness and per-LUN readable sensors |
| SDR-FIRMWARE-POPULATION | Rule 6.2-13 | 124 | NOT-TESTABLE | Target exposes no firmware-upgrade capability; cannot establish the conditional prerequisite from permitted texts |
| SENSOR-STATE | Rule 6.2-1; Table 10.2-1; Table 10.2.1-1 | 123; 236; 238 | PASS | Sensor 0 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-LINK | Rule 6.2-1; Table 10.2-1; Table 10.2.2-1 | 123; 236; 241-242 | PASS | Sensor 1 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-HEALTH | Rule 6.2-1; Table 10.2-1; Table 10.2.3-1 | 123; 236; 243 | PASS | Sensor 2 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-VOLTAGE | Rule 6.2-1; Table 10.2-1; Table 10.2.4-1 | 123; 236; 245 | PASS | Sensor 3 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEMPERATURE | Rule 6.2-1; Table 10.2-1; Table 10.2.5-1 | 123; 236; 246-247 | PASS | Sensor 4 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEST-RESULTS | Rule 6.2-1; Table 10.2-1; Table 10.2.6-1 | 123; 236; 248 | PASS | Sensor 5 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-TEST-STATUS | Rule 6.2-1; Table 10.2-1; Table 10.2.7-1 | 123; 236; 249 | PASS | Sensor 6 on FRU 0 LUN 0: type, event code, reading layout and SDR mask |
| SENSOR-UNASSIGNED | IPMI Table 35-15; Table 5-2, §5.3 | IPMI 497; 70-72 | PASS | Unknown sensor number yields CBh or the allowed generic unspecified error |
| SENSOR-MANDATORY-NUMBERS | Rule 6.2-2; §10.1.3.26; Rule 4.1.5.4.1-10 | 123; 207-208; 55 | NOT-TESTABLE | Table 10.1.2-1 pp.178-179 marks this optional; Rule 6.2-2 p.123 conflicts (review required) |
| SENSOR-TYPE-RESERVATIONS | Rule 6.2-12; Table 10.2-3 | 124; 237 | PASS | No sensor uses F9h..FFh reserved sensor types |
| SENSOR-TEMPERATURE-STIMULUS | §4.1.6; §4.1.7 | 62-64 | NOT-TESTABLE | Target has no temperature, voltage or payload-test stimulus controls; layouts and SDRs are tested separately |
| FRU-AREA-INFO | Rules 8.1-1/-5; IPMI Table 34-2 | 138-139; IPMI 476 | PASS | FRU 0 on LUN 0: exact inventory info layout and byte/word access encoding |
| FRU-READ | Rule 8.1-5; IPMI Table 34-3 | 139; IPMI 477 | PASS | Read entire inventory in bounded chunks; validate returned counts and byte-exact overlapping reads |
| FRU-READ-OFFSET | IPMI Table 34-3; Table 5-2, §5.3 | IPMI 477; 70-72 | PASS | Read starting past the FRU size rejected with out-of-range or permitted unspecified error |
| FRU-READ-COUNT | IPMI Table 34-3; Table 5-2, §5.3 | IPMI 477; 70-72 | PASS | Reject zero read count; oversized reads return bounded data or a permitted transfer-size error |
| FRU-COMMON-HEADER | Rule 8.1-2; FRU Table 8-1 | 138; FRU 5 | PASS | Common header version, offsets, mandatory Board/Product areas, pad and checksum |
| FRU-NO-CHASSIS-AREA | Rule 8.1-3 | 139 | PASS | Board FRU common header has zero Chassis Info Area offset |
| FRU-BOARD-AREA | Rule 8.1-2; FRU Table 11-1 | 138; FRU 7 | PASS | BOARD area version, size, checksum, required type/length fields, terminator and padding |
| FRU-PRODUCT-AREA | Rule 8.1-2; FRU Table 12-1 | 138; FRU 8 | PASS | PRODUCT area version, size, checksum, required type/length fields, terminator and padding |
| FRU-MULTIRECORD | FRU §§16-17; VITA Table 10.3-1 | FRU 13-15; 255 | NOT-TESTABLE | Optional MultiRecord area is absent; permitted by Permission 8.1-4 p.139 |
| FRU-WRITE | Rule 8.1-5; IPMI Table 34-4; Rules 8-4/-5 | 139; IPMI 478; 137 | PASS | Write original header bytes, validate count or 80h protection, restore original bytes and verify full image |
| FRU-WRITE-OFFSET | IPMI Table 34-4; Table 5-2, §5.3 | IPMI 478; 70-72 | PASS | Reject out-of-range write; original full image restored and verified even after failure |
| FRU-POWER-INDEPENDENCE | Rule 8-1 | 136 | NOT-TESTABLE | Target cannot control or independently measure payload power; FRU reads cover only current power state |
| FRU-CHECKSUM-INITIATOR | Rule 8-2 | 137 | NOT-TESTABLE | A deliberate invalid-checksum write conflicts with the required checksum-preserving test policy; valid images and writes are covered |
| LINK-A-ACCESS | Rules 4.1.1-1/-5; 9.2.1.1-1 | 26; 28; 160 | PASS | Implemented mandatory IPMB-A accepts a valid request |
| LINK-STATE-NO-CHANGE | §10.1.3.7; Rule 9.3-10 | 189-190; 170 | PASS | Set IPMB State FFh no-change leaves override and speed fields unchanged |
| LINK-REFUSE-ISOLATION | Rule 4.1.8-3 | 68 | PASS | Refuse isolation of both interfaces and remain reachable without changing overrides |
| LINK-UNSUPPORTED-ATOMIC | Rule 4.1.1-14; §10.1.3.7 | 31; 189-190 | PASS | Unsupported link/speed parameters return CCh and make no change |
| LINK-OPTIONAL-SPEED | Table 3.3-1; §10.1.3.7 | 24-25; 189-190 | PASS | 2022 controller accepts the optional speed byte at current supported speed |
| LINK-BOOT | Rules 4.1.1-4/-4.3; 4.1.8-1 | 27-28; 68 | PASS | Following reset, A is enabled, B disabled, and speed is STANDARD |
| LINK-B-ACCESS | Rule 4.1.1-5 | 28 | PASS | When B is implemented/enabled, both A and B accept requests equally |
| LINK-ALTERNATION | Rule 4.1.1-6 | 28 | PASS | With two enabled healthy interfaces, successive IPMC transmissions alternate buses |
| LINK-ISOLATION-LOCAL | §10.1.3.7; Rules 9.3-2/-4 | 189-190; 168-169 | PASS | Isolate A while B stays local; sensor reports overrides/enable state, restore A then isolate B |
| LINK-FAILOVER | Rule 4.1.1-7 | 28 | PASS | When the next transmit bus fails, receive the response over the enabled alternate bus |
| LINK-INFO-OPTIONAL | Rules 4.1.8-4/-5; §10.1.3.17 | 68; 200 | NOT-TESTABLE | Optional command is unsupported (C1h), a compliant Tier-1 choice |
| LINK-FAILOVER-ATTEMPT | Rule 4.1.1-7 | 28 | NOT-TESTABLE | Target reports only received frames; bus_down loses frames and exposes neither failed transmit attempts nor acknowledgement status |
| LINK-UNKNOWN-PEER-B | Rule 4.1.8-2 | 68 | NOT-TESTABLE | Tier-1 generates no events; Target cannot cause arbitrary outbound requests or configure peer capability knowledge |
| LINK-PERSISTENT-FAULT | §9.3 | 167-169 | NOT-TESTABLE | bus_down drops frames; it does not drive clock/data electrical fault levels or expose buffer diagnostic controls; extract gives no diagnostic timeout |
| TIMING-T5 | Rule 4.1.1-11; Table 4.1.1-1 T5 | 30-31 | PASS | 227 ms STOP-to-STOP deadline per request, with no adapter retries masking slow responses |
| TIMING-T5-RAW | Rule 4.1.1-11; Table 4.1.1-1 T5 | 30-31 | NOT-TESTABLE | send_frame/receive return neither request STOP nor response STOP timestamps; host elapsed time includes I/O latency and is not a T5 measurement |
| TIMING-T1-T2-T7-T8-T10 | Table 4.1.1-1 | 31 | NOT-TESTABLE | Target has no electrical edge timestamps, arbitration/bus-free instrumentation or processing lockout observation |
| TIMING-OUTBOUND-RETRY | Rules 4.1.1-8/-9/-10; Table 4.1.1-1 T3/T6/C1 | 29-31 | NOT-TESTABLE | Target cannot trigger arbitrary IPMC-originated requests; Tier-1 is forbidden to generate events; adapter retries would measure the requester instead |
| TIMING-T9-EXPIRATION | Table 4.1.1-1 T9; Obs.4.1.1-6 | 30-31 | NOT-TESTABLE | No observable non-idempotent write or request-cache introspection; identical replies cannot establish cache expiration; runner avoids incidental reuse within five seconds |
| BROADCAST-ADDRESSED | Table 10.1.1-1; IPMI §20.9, Fig.20-1 | 172; IPMI 280 | PASS | General-call prefix plus embedded own address gives exactly one correctly sourced Device ID response |
| BROADCAST-OTHER-ADDRESS | IPMI §20.9, Fig.20-1 | IPMI 280 | PASS | Controller ignores a broadcast with an embedded address other than its own |
| FRAME-BAD-HEADER-CHECKSUM | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore bad header checksum frame and remain responsive |
| FRAME-BAD-DATA-CHECKSUM | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore bad data checksum frame and remain responsive |
| FRAME-SHORT | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore short frame and remain responsive |
| FRAME-WRONG-DESTINATION | Rule 2.1-1; IPMI message framing | 21; IPMI 163-164; 280 | PASS | Ignore wrong destination frame and remain responsive |
| DUPLICATE-READ | IPMI Table 5-2, §5.3; Obs.4.1.1-6 | IPMI 70-72; 30 | PASS | Identical Device ID request and sequence receive an equivalent response or legal duplicate error |
| DUPLICATE-WRITE-REPLY | IPMI Table 5-2; Table 34-4; Obs.4.1.1-6 | IPMI 70; 478; 30 | PASS | Repeat an identical checksum-preserving FRU write with the same sequence; restore and verify image |
| EVENT-NO-GENERATION | Rule 7-4 | 134 | PASS | Observe a bounded quiet capture window and reject any IPMC Platform Event request |
| DUPLICATE-WRITE-NO-REEXECUTION | IPMI Table 5-2; Obs.4.1.1-6 | IPMI 70; 30 | NOT-TESTABLE | Target exposes no write counter or non-idempotent safe write; equal FRU bytes cannot prove execution count. Supplied texts explicitly suppress duplicate event logging, not arbitrary FRU writes; CFh is permitted for duplicate replies |
| EVENT-NO-GENERATION-STIMULATED | Rule 7-4 | 134 | NOT-TESTABLE | Target cannot stimulate hardware sensor/power transitions; bounded idle capture and event-disabled reading flags are tested |
| PROTECTION-OFFSETS | Rules 8-4/-5; IPMI Table 34-4 | 137; IPMI 478 | NOT-TESTABLE | No protected byte in current Common/Board/Product areas; 80h behavior cannot be induced through Target |
| NVMRO-ASSERTED | Rule 8-3; §4.1.10 | 137; 70-71 | NOT-TESTABLE | Target exposes neither NVMRO state nor assertion/deassertion control; a protected FRU alone does not establish NVMRO causation |
| NVMRO-OTHER-NVM | Recs.4.1.10-1/-2 | 70-71 | NOT-TESTABLE | Recommendations are not mandatory Tier-1 failures; Target has no NVMRO control or generic nonvolatile-memory access |
| PROTECTION-COMMAND-DEPENDENCY | Rule 4.1.17-1; §§10.1.3.34-36 | 88; 217-220 | NOT-TESTABLE | Permitted extract gives command IDs and dependency but omits request/response layouts; cannot issue a valid setter or interpret the capability masks independently |
| PROTECTION-INTERNAL-MULTIRECORD | Rules 8-4/-5 | 137 | NOT-TESTABLE | Target has no way to configure protection maps; optional internal data must not be altered speculatively. Common/Board/Product current-map probes are separate |

## CMD-OPTIONAL-POLICY: observed frames

```text
TX A nf=2c cmd=0b lun=0 seq=51 data=03 00
RX A 14.941 ms 20 b4 2c 82 cc 0b c1 03 e3
```

## CMD-OPTIONAL-CONTROL-CAP: observed frames

```text
TX A nf=2c cmd=1e lun=0 seq=52 data=03 00
RX A 15.717 ms 20 b4 2c 82 d0 1e c1 03 cc
TX A nf=2c cmd=04 lun=0 seq=53 data=03 00 ff
RX A 15.657 ms 20 b4 2c 82 d4 04 c1 03 e2
TX A nf=2c cmd=0c lun=0 seq=54 data=03 00 ff
RX A 16.125 ms 20 b4 2c 82 d8 0c c1 03 d6
```

## SENSOR-MANDATORY-NUMBERS: observed frames

```text
TX A nf=04 cmd=2d lun=0 seq=53 data=00
RX A 15.481 ms 20 14 cc 82 d4 2d 00 00 40 10 2d
TX A nf=2c cmd=44 lun=0 seq=54 data=03 00
RX A 16.053 ms 20 b4 2c 82 d8 44 c1 03 9e
```

## LINK-INFO-OPTIONAL: observed frames

```text
TX A nf=2c cmd=18 lun=0 seq=46 data=03 00 00
RX A 15.621 ms 20 b4 2c 82 b8 18 c1 03 ea
```

## PROTECTION-OFFSETS: observed frames

```text
TX A nf=0a cmd=10 lun=0 seq=5 data=00
RX A 20.657 ms 20 2c b4 82 14 10 00 00 01 00 59
TX A nf=0a cmd=11 lun=0 seq=6 data=00 00 00 10
RX A 16.005 ms 20 2c b4 82 18 11 00 10 01 00 00 01 0b 00 00 f3 01 0a 00 20 42 f7 ca 49 ce
TX A nf=0a cmd=11 lun=0 seq=7 data=00 10 00 10
RX A 15.892 ms 20 2c b4 82 1c 11 00 10 72 6f 6e 4b 65 72 6e 65 6c cf 56 50 58 20 54 69 e7
TX A nf=0a cmd=11 lun=0 seq=8 data=00 20 00 10
RX A 15.820 ms 20 2c b4 82 20 11 00 10 65 72 2d 31 20 49 50 4d 43 cc 49 4b 2d 54 31 2d 80
TX A nf=0a cmd=11 lun=0 seq=9 data=00 30 00 10
RX A 15.758 ms 20 2c b4 82 24 11 00 10 30 30 30 30 30 31 ce 49 4b 2d 49 50 4d 43 2d 54 df
TX A nf=0a cmd=11 lun=0 seq=10 data=00 40 00 10
RX A 16.043 ms 20 2c b4 82 28 11 00 10 31 2d 4e 55 43 cb 69 70 6d 63 2d 66 72 75 2d 76 60
TX A nf=0a cmd=11 lun=0 seq=11 data=00 50 00 10
RX A 15.731 ms 20 2c b4 82 2c 11 00 10 31 c1 00 00 00 00 00 51 01 09 00 ca 49 72 6f 6e 82
TX A nf=0a cmd=11 lun=0 seq=12 data=00 60 00 10
RX A 15.859 ms 20 2c b4 82 30 11 00 10 4b 65 72 6e 65 6c d5 54 69 65 72 2d 31 20 49 50 4c
TX A nf=0a cmd=11 lun=0 seq=13 data=00 70 00 10
RX A 15.895 ms 20 2c b4 82 34 11 00 10 4d 43 20 52 65 66 65 72 65 6e 63 65 ca 49 4b 2d 5f
TX A nf=0a cmd=11 lun=0 seq=14 data=00 80 00 10
RX A 15.670 ms 20 2c b4 82 38 11 00 10 49 50 4d 43 2d 54 31 c3 30 2e 31 cc 49 4b 2d 54 17
TX A nf=0a cmd=11 lun=0 seq=15 data=00 90 00 10
RX A 16.293 ms 20 2c b4 82 3c 11 00 10 31 2d 30 30 30 30 30 31 c0 c0 c1 00 00 00 00 1b 46
TX A nf=0a cmd=11 lun=0 seq=16 data=00 a0 00 10
RX A 15.582 ms 20 2c b4 82 40 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 1d
TX A nf=0a cmd=11 lun=0 seq=17 data=00 b0 00 10
RX A 15.704 ms 20 2c b4 82 44 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 19
TX A nf=0a cmd=11 lun=0 seq=18 data=00 c0 00 10
RX A 15.744 ms 20 2c b4 82 48 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 15
TX A nf=0a cmd=11 lun=0 seq=19 data=00 d0 00 10
RX A 15.850 ms 20 2c b4 82 4c 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 11
TX A nf=0a cmd=11 lun=0 seq=20 data=00 e0 00 10
RX A 15.876 ms 20 2c b4 82 50 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 0d
TX A nf=0a cmd=11 lun=0 seq=21 data=00 f0 00 10
RX A 15.699 ms 20 2c b4 82 54 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 09
TX A nf=0a cmd=10 lun=0 seq=22 data=00
RX A 15.692 ms 20 2c b4 82 58 10 00 00 01 00 15
TX A nf=0a cmd=12 lun=0 seq=23 data=00 00 00 01
RX A 15.807 ms 20 2c b4 82 5c 12 00 01 0f
TX A nf=0a cmd=12 lun=0 seq=24 data=00 01 00 00
RX A 15.929 ms 20 2c b4 82 60 12 00 01 0b
TX A nf=0a cmd=12 lun=0 seq=25 data=00 02 00 00
RX A 15.878 ms 20 2c b4 82 64 12 00 01 07
TX A nf=0a cmd=12 lun=0 seq=26 data=00 03 00 01
RX A 15.966 ms 20 2c b4 82 68 12 00 01 03
TX A nf=0a cmd=12 lun=0 seq=27 data=00 04 00 0b
RX A 16.083 ms 20 2c b4 82 6c 12 00 01 ff
TX A nf=0a cmd=12 lun=0 seq=28 data=00 05 00 00
RX A 15.704 ms 20 2c b4 82 70 12 00 01 fb
TX A nf=0a cmd=12 lun=0 seq=29 data=00 06 00 00
RX A 15.973 ms 20 2c b4 82 74 12 00 01 f7
TX A nf=0a cmd=12 lun=0 seq=30 data=00 07 00 f3
RX A 15.883 ms 20 2c b4 82 78 12 00 01 f3
TX A nf=0a cmd=12 lun=0 seq=31 data=00 08 00 01
RX A 15.544 ms 20 2c b4 82 7c 12 00 01 ef
TX A nf=0a cmd=12 lun=0 seq=32 data=00 09 00 0a
RX A 16.337 ms 20 2c b4 82 80 12 00 01 eb
TX A nf=0a cmd=12 lun=0 seq=33 data=00 0a 00 00
RX A 15.623 ms 20 2c b4 82 84 12 00 01 e7
TX A nf=0a cmd=12 lun=0 seq=34 data=00 0b 00 20
RX A 15.342 ms 20 2c b4 82 88 12 00 01 e3
TX A nf=0a cmd=12 lun=0 seq=35 data=00 0c 00 42
RX A 16.869 ms 20 2c b4 82 8c 12 00 01 df
TX A nf=0a cmd=12 lun=0 seq=36 data=00 0d 00 f7
RX A 14.692 ms 20 2c b4 82 90 12 00 01 db
TX A nf=0a cmd=12 lun=0 seq=37 data=00 0e 00 ca
RX A 15.206 ms 20 2c b4 82 94 12 00 01 d7
TX A nf=0a cmd=12 lun=0 seq=38 data=00 0f 00 49
RX A 15.375 ms 20 2c b4 82 98 12 00 01 d3
TX A nf=0a cmd=12 lun=0 seq=39 data=00 10 00 72
RX A 15.535 ms 20 2c b4 82 9c 12 00 01 cf
TX A nf=0a cmd=12 lun=0 seq=40 data=00 11 00 6f
RX A 15.621 ms 20 2c b4 82 a0 12 00 01 cb
TX A nf=0a cmd=12 lun=0 seq=41 data=00 12 00 6e
RX A 27.356 ms 20 2c b4 82 a4 12 00 01 c7
TX A nf=0a cmd=12 lun=0 seq=42 data=00 13 00 4b
RX A 15.657 ms 20 2c b4 82 a8 12 00 01 c3
TX A nf=0a cmd=12 lun=0 seq=43 data=00 14 00 65
RX A 15.607 ms 20 2c b4 82 ac 12 00 01 bf
TX A nf=0a cmd=12 lun=0 seq=44 data=00 15 00 72
RX A 15.283 ms 20 2c b4 82 b0 12 00 01 bb
TX A nf=0a cmd=12 lun=0 seq=45 data=00 16 00 6e
RX A 14.717 ms 20 2c b4 82 b4 12 00 01 b7
TX A nf=0a cmd=12 lun=0 seq=46 data=00 17 00 65
RX A 15.639 ms 20 2c b4 82 b8 12 00 01 b3
TX A nf=0a cmd=12 lun=0 seq=47 data=00 18 00 6c
RX A 15.717 ms 20 2c b4 82 bc 12 00 01 af
TX A nf=0a cmd=12 lun=0 seq=48 data=00 19 00 cf
RX A 17.708 ms 20 2c b4 82 c0 12 00 01 ab
TX A nf=0a cmd=12 lun=0 seq=49 data=00 1a 00 56
RX A 7.630 ms 20 2c b4 82 c4 12 00 01 a7
TX A nf=0a cmd=12 lun=0 seq=50 data=00 1b 00 50
RX A 15.621 ms 20 2c b4 82 c8 12 00 01 a3
TX A nf=0a cmd=12 lun=0 seq=51 data=00 1c 00 58
RX A 15.898 ms 20 2c b4 82 cc 12 00 01 9f
TX A nf=0a cmd=12 lun=0 seq=52 data=00 1d 00 20
RX A 15.744 ms 20 2c b4 82 d0 12 00 01 9b
TX A nf=0a cmd=12 lun=0 seq=53 data=00 1e 00 54
RX A 15.491 ms 20 2c b4 82 d4 12 00 01 97
TX A nf=0a cmd=12 lun=0 seq=54 data=00 1f 00 69
RX A 16.098 ms 20 2c b4 82 d8 12 00 01 93
TX A nf=0a cmd=12 lun=0 seq=55 data=00 20 00 65
RX A 15.944 ms 20 2c b4 82 dc 12 00 01 8f
TX A nf=0a cmd=12 lun=0 seq=56 data=00 21 00 72
RX A 15.784 ms 20 2c b4 82 e0 12 00 01 8b
TX A nf=0a cmd=12 lun=0 seq=57 data=00 22 00 2d
RX A 15.771 ms 20 2c b4 82 e4 12 00 01 87
TX A nf=0a cmd=12 lun=0 seq=58 data=00 23 00 31
RX A 15.543 ms 20 2c b4 82 e8 12 00 01 83
TX A nf=0a cmd=12 lun=0 seq=59 data=00 24 00 20
RX A 15.980 ms 20 2c b4 82 ec 12 00 01 7f
TX A nf=0a cmd=12 lun=0 seq=60 data=00 25 00 49
RX A 15.779 ms 20 2c b4 82 f0 12 00 01 7b
TX A nf=0a cmd=12 lun=0 seq=61 data=00 26 00 50
RX A 15.057 ms 20 2c b4 82 f4 12 00 01 77
TX A nf=0a cmd=12 lun=0 seq=62 data=00 27 00 4d
RX A 15.302 ms 20 2c b4 82 f8 12 00 01 73
TX A nf=0a cmd=12 lun=0 seq=63 data=00 28 00 43
RX A 15.740 ms 20 2c b4 82 fc 12 00 01 6f
TX A nf=0a cmd=12 lun=0 seq=0 data=00 29 00 cc
RX A 12.764 ms 20 2c b4 82 00 12 00 01 6b
TX A nf=0a cmd=12 lun=0 seq=1 data=00 2a 00 49
RX A 12.099 ms 20 2c b4 82 04 12 00 01 67
TX A nf=0a cmd=12 lun=0 seq=2 data=00 2b 00 4b
RX A 11.546 ms 20 2c b4 82 08 12 00 01 63
TX A nf=0a cmd=12 lun=0 seq=3 data=00 2c 00 2d
RX A 5.514 ms 20 2c b4 82 0c 12 00 01 5f
TX A nf=0a cmd=12 lun=0 seq=4 data=00 2d 00 54
RX A 2.202 ms 20 2c b4 82 10 12 00 01 5b
TX A nf=0a cmd=12 lun=0 seq=5 data=00 2e 00 31
RX A 16.159 ms 20 2c b4 82 14 12 00 01 57
TX A nf=0a cmd=12 lun=0 seq=6 data=00 2f 00 2d
RX A 11.460 ms 20 2c b4 82 18 12 00 01 53
TX A nf=0a cmd=12 lun=0 seq=7 data=00 30 00 30
RX A 10.742 ms 20 2c b4 82 1c 12 00 01 4f
TX A nf=0a cmd=12 lun=0 seq=8 data=00 31 00 30
RX A 10.547 ms 20 2c b4 82 20 12 00 01 4b
TX A nf=0a cmd=12 lun=0 seq=9 data=00 32 00 30
RX A 11.031 ms 20 2c b4 82 24 12 00 01 47
TX A nf=0a cmd=12 lun=0 seq=10 data=00 33 00 30
RX A 10.801 ms 20 2c b4 82 28 12 00 01 43
TX A nf=0a cmd=12 lun=0 seq=11 data=00 34 00 30
RX A 11.029 ms 20 2c b4 82 2c 12 00 01 3f
TX A nf=0a cmd=12 lun=0 seq=12 data=00 35 00 31
RX A 10.960 ms 20 2c b4 82 30 12 00 01 3b
TX A nf=0a cmd=12 lun=0 seq=13 data=00 36 00 ce
RX A 11.196 ms 20 2c b4 82 34 12 00 01 37
TX A nf=0a cmd=12 lun=0 seq=14 data=00 37 00 49
RX A 10.916 ms 20 2c b4 82 38 12 00 01 33
TX A nf=0a cmd=12 lun=0 seq=15 data=00 38 00 4b
RX A 11.137 ms 20 2c b4 82 3c 12 00 01 2f
TX A nf=0a cmd=12 lun=0 seq=16 data=00 39 00 2d
RX A 10.473 ms 20 2c b4 82 40 12 00 01 2b
TX A nf=0a cmd=12 lun=0 seq=17 data=00 3a 00 49
RX A 10.803 ms 20 2c b4 82 44 12 00 01 27
TX A nf=0a cmd=12 lun=0 seq=18 data=00 3b 00 50
RX A 10.837 ms 20 2c b4 82 48 12 00 01 23
TX A nf=0a cmd=12 lun=0 seq=19 data=00 3c 00 4d
RX A 10.979 ms 20 2c b4 82 4c 12 00 01 1f
TX A nf=0a cmd=12 lun=0 seq=20 data=00 3d 00 43
RX A 11.598 ms 20 2c b4 82 50 12 00 01 1b
TX A nf=0a cmd=12 lun=0 seq=21 data=00 3e 00 2d
RX A 10.780 ms 20 2c b4 82 54 12 00 01 17
TX A nf=0a cmd=12 lun=0 seq=22 data=00 3f 00 54
RX A 11.031 ms 20 2c b4 82 58 12 00 01 13
TX A nf=0a cmd=12 lun=0 seq=23 data=00 40 00 31
RX A 10.237 ms 20 2c b4 82 5c 12 00 01 0f
TX A nf=0a cmd=12 lun=0 seq=24 data=00 41 00 2d
RX A 16.118 ms 20 2c b4 82 60 12 00 01 0b
TX A nf=0a cmd=12 lun=0 seq=25 data=00 42 00 4e
RX A 15.441 ms 20 2c b4 82 64 12 00 01 07
TX A nf=0a cmd=12 lun=0 seq=26 data=00 43 00 55
RX A 15.529 ms 20 2c b4 82 68 12 00 01 03
TX A nf=0a cmd=12 lun=0 seq=27 data=00 44 00 43
RX A 14.439 ms 20 2c b4 82 6c 12 00 01 ff
TX A nf=0a cmd=12 lun=0 seq=28 data=00 45 00 cb
RX A 14.367 ms 20 2c b4 82 70 12 00 01 fb
TX A nf=0a cmd=12 lun=0 seq=29 data=00 46 00 69
RX A 14.935 ms 20 2c b4 82 74 12 00 01 f7
TX A nf=0a cmd=12 lun=0 seq=30 data=00 47 00 70
RX A 14.619 ms 20 2c b4 82 78 12 00 01 f3
TX A nf=0a cmd=12 lun=0 seq=31 data=00 48 00 6d
RX A 14.166 ms 20 2c b4 82 7c 12 00 01 ef
TX A nf=0a cmd=12 lun=0 seq=32 data=00 49 00 63
RX A 14.351 ms 20 2c b4 82 80 12 00 01 eb
TX A nf=0a cmd=12 lun=0 seq=33 data=00 4a 00 2d
RX A 12.252 ms 20 2c b4 82 84 12 00 01 e7
TX A nf=0a cmd=12 lun=0 seq=34 data=00 4b 00 66
RX A 11.906 ms 20 2c b4 82 88 12 00 01 e3
TX A nf=0a cmd=12 lun=0 seq=35 data=00 4c 00 72
RX A 11.837 ms 20 2c b4 82 8c 12 00 01 df
TX A nf=0a cmd=12 lun=0 seq=36 data=00 4d 00 75
RX A 10.623 ms 20 2c b4 82 90 12 00 01 db
TX A nf=0a cmd=12 lun=0 seq=37 data=00 4e 00 2d
RX A 6.975 ms 20 2c b4 82 94 12 00 01 d7
TX A nf=0a cmd=12 lun=0 seq=38 data=00 4f 00 76
RX A 7.207 ms 20 2c b4 82 98 12 00 01 d3
TX A nf=0a cmd=12 lun=0 seq=39 data=00 50 00 31
RX A 6.555 ms 20 2c b4 82 9c 12 00 01 cf
TX A nf=0a cmd=12 lun=0 seq=40 data=00 51 00 c1
RX A 6.362 ms 20 2c b4 82 a0 12 00 01 cb
TX A nf=0a cmd=12 lun=0 seq=41 data=00 52 00 00
RX A 5.343 ms 20 2c b4 82 a4 12 00 01 c7
TX A nf=0a cmd=12 lun=0 seq=42 data=00 53 00 00
RX A 5.028 ms 20 2c b4 82 a8 12 00 01 c3
TX A nf=0a cmd=12 lun=0 seq=43 data=00 54 00 00
RX A 5.094 ms 20 2c b4 82 ac 12 00 01 bf
TX A nf=0a cmd=12 lun=0 seq=44 data=00 55 00 00
RX A 5.723 ms 20 2c b4 82 b0 12 00 01 bb
TX A nf=0a cmd=12 lun=0 seq=45 data=00 56 00 00
RX A 6.083 ms 20 2c b4 82 b4 12 00 01 b7
TX A nf=0a cmd=12 lun=0 seq=46 data=00 57 00 51
RX A 7.105 ms 20 2c b4 82 b8 12 00 01 b3
TX A nf=0a cmd=12 lun=0 seq=47 data=00 58 00 01
RX A 6.809 ms 20 2c b4 82 bc 12 00 01 af
TX A nf=0a cmd=12 lun=0 seq=48 data=00 59 00 09
RX A 7.015 ms 20 2c b4 82 c0 12 00 01 ab
TX A nf=0a cmd=12 lun=0 seq=49 data=00 5a 00 00
RX A 3.965 ms 20 2c b4 82 c4 12 00 01 a7
TX A nf=0a cmd=12 lun=0 seq=50 data=00 5b 00 ca
RX A 11.942 ms 20 2c b4 82 c8 12 00 01 a3
TX A nf=0a cmd=12 lun=0 seq=51 data=00 5c 00 49
RX A 12.531 ms 20 2c b4 82 cc 12 00 01 9f
TX A nf=0a cmd=12 lun=0 seq=52 data=00 5d 00 72
RX A 12.210 ms 20 2c b4 82 d0 12 00 01 9b
TX A nf=0a cmd=12 lun=0 seq=53 data=00 5e 00 6f
RX A 11.767 ms 20 2c b4 82 d4 12 00 01 97
TX A nf=0a cmd=12 lun=0 seq=54 data=00 5f 00 6e
RX A 11.400 ms 20 2c b4 82 d8 12 00 01 93
TX A nf=0a cmd=12 lun=0 seq=55 data=00 60 00 4b
RX A 11.347 ms 20 2c b4 82 dc 12 00 01 8f
TX A nf=0a cmd=12 lun=0 seq=56 data=00 61 00 65
RX A 15.695 ms 20 2c b4 82 e0 12 00 01 8b
TX A nf=0a cmd=12 lun=0 seq=57 data=00 62 00 72
RX A 15.708 ms 20 2c b4 82 e4 12 00 01 87
TX A nf=0a cmd=12 lun=0 seq=58 data=00 63 00 6e
RX A 15.205 ms 20 2c b4 82 e8 12 00 01 83
TX A nf=0a cmd=12 lun=0 seq=59 data=00 64 00 65
RX A 16.056 ms 20 2c b4 82 ec 12 00 01 7f
TX A nf=0a cmd=12 lun=0 seq=60 data=00 65 00 6c
RX A 15.776 ms 20 2c b4 82 f0 12 00 01 7b
TX A nf=0a cmd=12 lun=0 seq=61 data=00 66 00 d5
RX A 15.669 ms 20 2c b4 82 f4 12 00 01 77
TX A nf=0a cmd=12 lun=0 seq=62 data=00 67 00 54
RX A 14.945 ms 20 2c b4 82 f8 12 00 01 73
TX A nf=0a cmd=12 lun=0 seq=63 data=00 68 00 69
RX A 16.548 ms 20 2c b4 82 fc 12 00 01 6f
TX A nf=0a cmd=12 lun=0 seq=0 data=00 69 00 65
RX A 13.356 ms 20 2c b4 82 00 12 00 01 6b
TX A nf=0a cmd=12 lun=0 seq=1 data=00 6a 00 72
RX A 14.054 ms 20 2c b4 82 04 12 00 01 67
TX A nf=0a cmd=12 lun=0 seq=2 data=00 6b 00 2d
RX A 13.876 ms 20 2c b4 82 08 12 00 01 63
TX A nf=0a cmd=12 lun=0 seq=3 data=00 6c 00 31
RX A 8.415 ms 20 2c b4 82 0c 12 00 01 5f
TX A nf=0a cmd=12 lun=0 seq=4 data=00 6d 00 20
RX A 10.306 ms 20 2c b4 82 10 12 00 01 5b
TX A nf=0a cmd=12 lun=0 seq=5 data=00 6e 00 49
RX A 15.293 ms 20 2c b4 82 14 12 00 01 57
TX A nf=0a cmd=12 lun=0 seq=6 data=00 6f 00 50
RX A 10.136 ms 20 2c b4 82 18 12 00 01 53
TX A nf=0a cmd=12 lun=0 seq=7 data=00 70 00 4d
RX A 8.246 ms 20 2c b4 82 1c 12 00 01 4f
TX A nf=0a cmd=12 lun=0 seq=8 data=00 71 00 43
RX A 7.785 ms 20 2c b4 82 20 12 00 01 4b
TX A nf=0a cmd=12 lun=0 seq=9 data=00 72 00 20
RX A 14.512 ms 20 2c b4 82 24 12 00 01 47
TX A nf=0a cmd=12 lun=0 seq=10 data=00 73 00 52
RX A 14.407 ms 20 2c b4 82 28 12 00 01 43
TX A nf=0a cmd=12 lun=0 seq=11 data=00 74 00 65
RX A 14.331 ms 20 2c b4 82 2c 12 00 01 3f
TX A nf=0a cmd=12 lun=0 seq=12 data=00 75 00 66
RX A 13.819 ms 20 2c b4 82 30 12 00 01 3b
TX A nf=0a cmd=12 lun=0 seq=13 data=00 76 00 65
RX A 14.274 ms 20 2c b4 82 34 12 00 01 37
TX A nf=0a cmd=12 lun=0 seq=14 data=00 77 00 72
RX A 12.844 ms 20 2c b4 82 38 12 00 01 33
TX A nf=0a cmd=12 lun=0 seq=15 data=00 78 00 65
RX A 14.017 ms 20 2c b4 82 3c 12 00 01 2f
TX A nf=0a cmd=12 lun=0 seq=16 data=00 79 00 6e
RX A 13.292 ms 20 2c b4 82 40 12 00 01 2b
TX A nf=0a cmd=12 lun=0 seq=17 data=00 7a 00 63
RX A 13.165 ms 20 2c b4 82 44 12 00 01 27
TX A nf=0a cmd=12 lun=0 seq=18 data=00 7b 00 65
RX A 13.412 ms 20 2c b4 82 48 12 00 01 23
TX A nf=0a cmd=12 lun=0 seq=19 data=00 7c 00 ca
RX A 13.946 ms 20 2c b4 82 4c 12 00 01 1f
TX A nf=0a cmd=12 lun=0 seq=20 data=00 7d 00 49
RX A 12.742 ms 20 2c b4 82 50 12 00 01 1b
TX A nf=0a cmd=12 lun=0 seq=21 data=00 7e 00 4b
RX A 11.973 ms 20 2c b4 82 54 12 00 01 17
TX A nf=0a cmd=12 lun=0 seq=22 data=00 7f 00 2d
RX A 11.840 ms 20 2c b4 82 58 12 00 01 13
TX A nf=0a cmd=12 lun=0 seq=23 data=00 80 00 49
RX A 24.130 ms 20 2c b4 82 5c 12 00 01 0f
TX A nf=0a cmd=12 lun=0 seq=24 data=00 81 00 50
RX A 15.719 ms 20 2c b4 82 60 12 00 01 0b
TX A nf=0a cmd=12 lun=0 seq=25 data=00 82 00 4d
RX A 15.448 ms 20 2c b4 82 64 12 00 01 07
TX A nf=0a cmd=12 lun=0 seq=26 data=00 83 00 43
RX A 16.181 ms 20 2c b4 82 68 12 00 01 03
TX A nf=0a cmd=12 lun=0 seq=27 data=00 84 00 2d
RX A 15.399 ms 20 2c b4 82 6c 12 00 01 ff
TX A nf=0a cmd=12 lun=0 seq=28 data=00 85 00 54
RX A 15.714 ms 20 2c b4 82 70 12 00 01 fb
TX A nf=0a cmd=12 lun=0 seq=29 data=00 86 00 31
RX A 15.715 ms 20 2c b4 82 74 12 00 01 f7
TX A nf=0a cmd=12 lun=0 seq=30 data=00 87 00 c3
RX A 15.395 ms 20 2c b4 82 78 12 00 01 f3
TX A nf=0a cmd=12 lun=0 seq=31 data=00 88 00 30
RX A 15.602 ms 20 2c b4 82 7c 12 00 01 ef
TX A nf=0a cmd=12 lun=0 seq=32 data=00 89 00 2e
RX A 16.186 ms 20 2c b4 82 80 12 00 01 eb
TX A nf=0a cmd=12 lun=0 seq=33 data=00 8a 00 31
RX A 15.277 ms 20 2c b4 82 84 12 00 01 e7
TX A nf=0a cmd=12 lun=0 seq=34 data=00 8b 00 cc
RX A 16.095 ms 20 2c b4 82 88 12 00 01 e3
TX A nf=0a cmd=12 lun=0 seq=35 data=00 8c 00 49
RX A 15.392 ms 20 2c b4 82 8c 12 00 01 df
TX A nf=0a cmd=12 lun=0 seq=36 data=00 8d 00 4b
RX A 15.521 ms 20 2c b4 82 90 12 00 01 db
TX A nf=0a cmd=12 lun=0 seq=37 data=00 8e 00 2d
RX A 15.453 ms 20 2c b4 82 94 12 00 01 d7
TX A nf=0a cmd=12 lun=0 seq=38 data=00 8f 00 54
RX A 15.341 ms 20 2c b4 82 98 12 00 01 d3
TX A nf=0a cmd=12 lun=0 seq=39 data=00 90 00 31
RX A 15.441 ms 20 2c b4 82 9c 12 00 01 cf
TX A nf=0a cmd=12 lun=0 seq=40 data=00 91 00 2d
RX A 15.740 ms 20 2c b4 82 a0 12 00 01 cb
TX A nf=0a cmd=12 lun=0 seq=41 data=00 92 00 30
RX A 15.613 ms 20 2c b4 82 a4 12 00 01 c7
TX A nf=0a cmd=12 lun=0 seq=42 data=00 93 00 30
RX A 21.564 ms 20 2c b4 82 a8 12 00 01 c3
TX A nf=0a cmd=12 lun=0 seq=43 data=00 94 00 30
RX A 16.020 ms 20 2c b4 82 ac 12 00 01 bf
TX A nf=0a cmd=12 lun=0 seq=44 data=00 95 00 30
RX A 15.771 ms 20 2c b4 82 b0 12 00 01 bb
TX A nf=0a cmd=12 lun=0 seq=45 data=00 96 00 30
RX A 15.258 ms 20 2c b4 82 b4 12 00 01 b7
TX A nf=0a cmd=12 lun=0 seq=46 data=00 97 00 31
RX A 15.640 ms 20 2c b4 82 b8 12 00 01 b3
TX A nf=0a cmd=12 lun=0 seq=47 data=00 98 00 c0
RX A 16.059 ms 20 2c b4 82 bc 12 00 01 af
TX A nf=0a cmd=12 lun=0 seq=48 data=00 99 00 c0
RX A 15.800 ms 20 2c b4 82 c0 12 00 01 ab
TX A nf=0a cmd=12 lun=0 seq=49 data=00 9a 00 c1
RX A 16.025 ms 20 2c b4 82 c4 12 00 01 a7
TX A nf=0a cmd=12 lun=0 seq=50 data=00 9b 00 00
RX A 15.819 ms 20 2c b4 82 c8 12 00 01 a3
TX A nf=0a cmd=12 lun=0 seq=51 data=00 9c 00 00
RX A 16.147 ms 20 2c b4 82 cc 12 00 01 9f
TX A nf=0a cmd=12 lun=0 seq=52 data=00 9d 00 00
RX A 15.932 ms 20 2c b4 82 d0 12 00 01 9b
TX A nf=0a cmd=12 lun=0 seq=53 data=00 9e 00 00
RX A 15.657 ms 20 2c b4 82 d4 12 00 01 97
TX A nf=0a cmd=12 lun=0 seq=54 data=00 9f 00 1b
RX A 16.035 ms 20 2c b4 82 d8 12 00 01 93
TX A nf=0a cmd=10 lun=0 seq=55 data=00
RX A 15.403 ms 20 2c b4 82 dc 10 00 00 01 00 91
TX A nf=0a cmd=11 lun=0 seq=56 data=00 00 00 10
RX A 16.218 ms 20 2c b4 82 e0 11 00 10 01 00 00 01 0b 00 00 f3 01 0a 00 20 42 f7 ca 49 06
TX A nf=0a cmd=11 lun=0 seq=57 data=00 10 00 10
RX A 15.458 ms 20 2c b4 82 e4 11 00 10 72 6f 6e 4b 65 72 6e 65 6c cf 56 50 58 20 54 69 1f
TX A nf=0a cmd=11 lun=0 seq=58 data=00 20 00 10
RX A 15.238 ms 20 2c b4 82 e8 11 00 10 65 72 2d 31 20 49 50 4d 43 cc 49 4b 2d 54 31 2d b8
TX A nf=0a cmd=11 lun=0 seq=59 data=00 30 00 10
RX A 16.356 ms 20 2c b4 82 ec 11 00 10 30 30 30 30 30 31 ce 49 4b 2d 49 50 4d 43 2d 54 17
TX A nf=0a cmd=11 lun=0 seq=60 data=00 40 00 10
RX A 15.436 ms 20 2c b4 82 f0 11 00 10 31 2d 4e 55 43 cb 69 70 6d 63 2d 66 72 75 2d 76 98
TX A nf=0a cmd=11 lun=0 seq=61 data=00 50 00 10
RX A 16.013 ms 20 2c b4 82 f4 11 00 10 31 c1 00 00 00 00 00 51 01 09 00 ca 49 72 6f 6e ba
TX A nf=0a cmd=11 lun=0 seq=62 data=00 60 00 10
RX A 15.996 ms 20 2c b4 82 f8 11 00 10 4b 65 72 6e 65 6c d5 54 69 65 72 2d 31 20 49 50 84
TX A nf=0a cmd=11 lun=0 seq=63 data=00 70 00 10
RX A 15.527 ms 20 2c b4 82 fc 11 00 10 4d 43 20 52 65 66 65 72 65 6e 63 65 ca 49 4b 2d 97
TX A nf=0a cmd=11 lun=0 seq=0 data=00 80 00 10
RX A 10.882 ms 20 2c b4 82 00 11 00 10 49 50 4d 43 2d 54 31 c3 30 2e 31 cc 49 4b 2d 54 4f
TX A nf=0a cmd=11 lun=0 seq=1 data=00 90 00 10
RX A 4.717 ms 20 2c b4 82 04 11 00 10 31 2d 30 30 30 30 30 31 c0 c0 c1 00 00 00 00 1b 7e
TX A nf=0a cmd=11 lun=0 seq=2 data=00 a0 00 10
RX A 4.574 ms 20 2c b4 82 08 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 55
TX A nf=0a cmd=11 lun=0 seq=3 data=00 b0 00 10
RX A 14.816 ms 20 2c b4 82 0c 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 51
TX A nf=0a cmd=11 lun=0 seq=4 data=00 c0 00 10
RX A 5.016 ms 20 2c b4 82 10 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 4d
TX A nf=0a cmd=11 lun=0 seq=5 data=00 d0 00 10
RX A 16.237 ms 20 2c b4 82 14 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 49
TX A nf=0a cmd=11 lun=0 seq=6 data=00 e0 00 10
RX A 11.837 ms 20 2c b4 82 18 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 45
TX A nf=0a cmd=11 lun=0 seq=7 data=00 f0 00 10
RX A 10.117 ms 20 2c b4 82 1c 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 41
TX A nf=0a cmd=10 lun=0 seq=8 data=00
RX A 9.694 ms 20 2c b4 82 20 10 00 00 01 00 4d
TX A nf=0a cmd=10 lun=0 seq=9 data=00
RX A 9.203 ms 20 2c b4 82 24 10 00 00 01 00 49
TX A nf=0a cmd=11 lun=0 seq=10 data=00 00 00 10
RX A 20.260 ms 20 2c b4 82 28 11 00 10 01 00 00 01 0b 00 00 f3 01 0a 00 20 42 f7 ca 49 be
TX A nf=0a cmd=11 lun=0 seq=11 data=00 10 00 10
RX A 15.741 ms 20 2c b4 82 2c 11 00 10 72 6f 6e 4b 65 72 6e 65 6c cf 56 50 58 20 54 69 d7
TX A nf=0a cmd=11 lun=0 seq=12 data=00 20 00 10
RX A 15.927 ms 20 2c b4 82 30 11 00 10 65 72 2d 31 20 49 50 4d 43 cc 49 4b 2d 54 31 2d 70
TX A nf=0a cmd=11 lun=0 seq=13 data=00 30 00 10
RX A 16.014 ms 20 2c b4 82 34 11 00 10 30 30 30 30 30 31 ce 49 4b 2d 49 50 4d 43 2d 54 cf
TX A nf=0a cmd=11 lun=0 seq=14 data=00 40 00 10
RX A 15.386 ms 20 2c b4 82 38 11 00 10 31 2d 4e 55 43 cb 69 70 6d 63 2d 66 72 75 2d 76 50
TX A nf=0a cmd=11 lun=0 seq=15 data=00 50 00 10
RX A 15.615 ms 20 2c b4 82 3c 11 00 10 31 c1 00 00 00 00 00 51 01 09 00 ca 49 72 6f 6e 72
TX A nf=0a cmd=11 lun=0 seq=16 data=00 60 00 10
RX A 15.798 ms 20 2c b4 82 40 11 00 10 4b 65 72 6e 65 6c d5 54 69 65 72 2d 31 20 49 50 3c
TX A nf=0a cmd=11 lun=0 seq=17 data=00 70 00 10
RX A 15.886 ms 20 2c b4 82 44 11 00 10 4d 43 20 52 65 66 65 72 65 6e 63 65 ca 49 4b 2d 4f
TX A nf=0a cmd=11 lun=0 seq=18 data=00 80 00 10
RX A 16.215 ms 20 2c b4 82 48 11 00 10 49 50 4d 43 2d 54 31 c3 30 2e 31 cc 49 4b 2d 54 07
TX A nf=0a cmd=11 lun=0 seq=19 data=00 90 00 10
RX A 15.807 ms 20 2c b4 82 4c 11 00 10 31 2d 30 30 30 30 30 31 c0 c0 c1 00 00 00 00 1b 36
TX A nf=0a cmd=11 lun=0 seq=20 data=00 a0 00 10
RX A 15.746 ms 20 2c b4 82 50 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 0d
TX A nf=0a cmd=11 lun=0 seq=21 data=00 b0 00 10
RX A 15.368 ms 20 2c b4 82 54 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 09
TX A nf=0a cmd=11 lun=0 seq=22 data=00 c0 00 10
RX A 15.747 ms 20 2c b4 82 58 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 05
TX A nf=0a cmd=11 lun=0 seq=23 data=00 d0 00 10
RX A 15.974 ms 20 2c b4 82 5c 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 01
TX A nf=0a cmd=11 lun=0 seq=24 data=00 e0 00 10
RX A 12.316 ms 20 2c b4 82 60 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 fd
TX A nf=0a cmd=11 lun=0 seq=25 data=00 f0 00 10
RX A 11.330 ms 20 2c b4 82 64 11 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 f9
```

PASS: 102; FAIL: 0; NOT-TESTABLE: 55
