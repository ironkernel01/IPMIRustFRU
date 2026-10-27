# IPMI and VITA 46.11 commands by tier

Internal working document. Lists every command ANSI/VITA 46.11-2022 defines
for a Chassis Manager (ChMC) and an IPMC, whether each tier must support it,
and where its definition lives, so an implementer knows which page to open.
The last column records the decision for this repository's Tier-1 IPMC.

Source: ANSI/VITA 46.11-2022 (`References/IPMI_VITA_46_11_SPEC_AV46DOT11-2022.pdf`,
licensed, local only), Table 10.1.1-1 (IPMI commands, printed p.172-176),
Table 10.1.2-1 (VITA 46.11 VSO commands, p.177-180) and Table 10.1.2-2
(minimum privilege, p.181-182). The M/O/NA cells are generated from the
tables in the PDF, not retyped. Page numbers are the printed page numbers
of the standard (PDF page = printed page + 8).

Legend: **M** mandatory, **O** optional, **NA** not applicable (an IPMC
shall not implement it and returns C1h, Rules 4.2-2 and 4.1.1-16),
`[n]` footnote of the source table (section 4).

## 1. Tier-1 IPMC build list

The commands this IPMC implements: the Tier-1 mandatory set only (owner decision 10-23-2026). Every other request returns C1h; a
request addressed to a subsidiary FRU returns C9h (Rule 4.2-3).

<!-- generated: sync_ipmi_commands.py -->
| NetFn | Cmd | Command | Tier-1 IPMC | Defined in | Page | Status |
|---|---|---|---|---|---|---|
| 06h | 01h | Get Device ID | M | IPMI 1.5 &sect;17.1 | VITA table p.172 | implemented |
| 06h | 04h | Get Self Test Results | M | IPMI 1.5 &sect;17.4 | VITA table p.172 | implemented |
| 06h | 01h | Broadcast "Get Device ID"[3] | M | IPMI 1.5 &sect;17.9 | VITA table p.172 | implemented |
| 04h | 20h | Get Device SDR Info | M | IPMI 1.5 &sect;29.2 | VITA table p.174 | implemented |
| 04h | 21h | Get Device SDR | M[2] | IPMI 1.5 &sect;29.3 | VITA table p.174 | implemented |
| 04h | 22h | Reserve Device SDR Repository | M[2] | IPMI 1.5 &sect;29.4 | VITA table p.174 | implemented |
| 04h | 2Dh | Get Sensor Reading | M | IPMI 1.5 &sect;29.14 | VITA table p.174 | implemented |
| 0Ah | 10h | Get FRU Inventory Area Info | M | IPMI 1.5 &sect;28.1 | VITA table p.174 | implemented |
| 0Ah | 11h | Read FRU Data | M | IPMI 1.5 &sect;28.2 | VITA table p.174 | implemented |
| 0Ah | 12h | Write FRU Data | M | IPMI 1.5 &sect;28.3 | VITA table p.174 | implemented |
| 2Ch | 00h | Get VSO Capabilities | M | VITA &sect;10.1.3.1 | 182 | implemented |
| 2Ch | 09h | Set IPMB State | M | VITA &sect;10.1.3.7 | 189 | implemented |
| 2Ch | 0Dh | Get Device Locator Record ID | M | VITA &sect;10.1.3.11 | 194 | implemented |
| 2Ch | 40h | Get FRU Address Info | M | VITA &sect;10.1.3.3 | 186 | implemented |

<!-- end generated -->

VITA 46.11 commands use NetFn 2Ch (request) / 2Dh (response) with the VSO
identifier 03h as data byte 1 of the request and byte 2 of the response.

Mandatory sensors on FRU 0 (fixed numbers, VITA Table 10.2-1), read with
Get Sensor Reading (04h/2Dh):

| Sensor # | Sensor type | Name | Reading layout |
|---|---|---|---|
| 0 | F0h | FRU State | &sect;10.2.1, Table 10.2.1-1, p.238 |
| 1 | F1h | IPMB Link | &sect;10.2.2, Table 10.2.2-1, p.241 |
| 2 | F2h | FRU Health | &sect;10.2.3, Table 10.2.3-1, p.243 |
| 3 | 02h | FRU Voltage | &sect;10.2.4, Table 10.2.4-1, p.245 |
| 4 | F3h | FRU Temperature | &sect;10.2.5, Table 10.2.5-1, p.246 |
| 5 | F4h | Payload Test Results | &sect;10.2.6, Table 10.2.6-1, p.248 |
| 6 | F5h | Payload Test Status | &sect;10.2.7, Table 10.2.7-1, p.249 |
| 7 | F7h | Payload Mode | &sect;10.2.8 (Tier 3 only, not implemented) |

The IPMC also publishes its measured rails and the die temperature as analog sensors with Full
Sensor Records (IPMI v2.0 Table 43-1), numbers after the mandatory sensors (`IPMC_SDR_ANALOG_SENSORS`):

| Sensor # | Sensor type | Name | Unit |
|---|---|---|---|
| 08h..0Ch | 02h | the rail names (`IPMC_RAILn_NAME`) | Volts |
| 0Dh | 01h | IPMC Die Temp | degrees C |

## 2. IPMI commands (VITA 46.11 Table 10.1.1-1)

"IPMI ref" is the reference as printed in the VITA table: `1.5:17.1` is
IPMI v1.5 section 17.1, `2.0:35.17` is IPMI v2.0 section 35.17. The IPMI
specification is not in `References/`; section 5 maps the v1.5 chapters to
IPMI v2.0.

<!-- generated: sync_ipmi_commands.py -->
| Cmd | Command | IPMI ref | IPMC T1 | IPMC T2 | IPMC T3 | ChMC T1 | ChMC T2 | ChMC T3 | This IPMC (Tier 1) |
|---|---|---|---|---|---|---|---|---|---|
| | **IPM Device "Global" Commands** | | | | | | | | |
| 06h/00h | Reserved | NA | NA | NA | N/A | NA | NA | N/A | C1h (NA for Tier 1) |
| 06h/01h | Get Device ID | 1.5:17.1 | M | M | M | M | M | M | implemented (app::get_device_id; Get Device ID; IPMI 2.0 20.1) |
| 06h/02h | Cold Reset | 1.5:17.2 | O[2] | M[2] | M | O[2] | M[2] | M | C1h (optional, not implemented) |
| 06h/03h | Warm Reset | 1.5:17.3 | O | O | M | O | O | M | C1h (optional, not implemented) |
| 06h/04h | Get Self Test Results | 1.5:17.4 | M | M | M | M | M | M | implemented (app::get_self_test_results; Get Self Test Results; IPMI 2.0 20.4) |
| 06h/05h | Manufacturing Test On | 1.5:17.5 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/06h | Set ACPI Power State | 1.5:17.6 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/07h | Get ACPI Power State | 1.5:17.7 | O[2] | O[2] | O | O[2] | O[2] | O | C1h (optional, not implemented) |
| 06h/08h | Get Device GUID | 1.5:17.8 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/01h | Broadcast "Get Device ID"[3] | 1.5:17.9 | M | M | M | M | M | M | implemented (link::BROADCAST_GET_DEVICE_ID) |
| | **BMC Watchdog Timer Commands** | | | | | | | | |
| 06h/22h | Reset Watchdog Timer | 1.5:21.5 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/24h | Set Watchdog Timer | 1.5:21.6 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/25h | Get Watchdog Timer | 1.5:21.7 | O | O | O | O | O | O | C1h (optional, not implemented) |
| | **BMC Device and Messaging Commands** | | | | | | | | |
| 06h/2Eh | Set BMC Global Enables[6] | 1.5:18.1 | O | O | O/M[7] | O | O | M | C1h (optional, not implemented) |
| 06h/2Fh | Get BMC Global Enables | 1.5:18.2 | O | O | M | M | M | M | C1h (optional, not implemented) |
| 06h/30h | Clear Message Flags[6] | 1.5:18.3 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/31h | Get Message Flags | 1.5:18.4 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/32h | Enable Message Channel Receive[6] | 1.5:18.5 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/33h | Get Message[6] | 1.5:18.6 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/34h | Send Message | 1.5:18.7 | O | O | O/M[7] | M | M | M | C1h (optional, not implemented) |
| 06h/35h | Read Event Message Buffer[6] | 1.5:18.8 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 06h/36h | Get BT Interface Capabilities | 1.5:18.9 | O[2] | O[2] | O[2] | O[2] | O[2] | O | C1h (optional, not implemented) |
| 06h/52h | Master Write-Read | 1.5:18.10 | O[2] | O[2] | O/M[8] | O[2] | O[2] | O/M[8] | C1h (optional, not implemented) |
| 06h/37h | Get System GUID | 1.5:18.13 | O[2] | O[2] | O[2] | O[2] | O[2] | O | C1h (optional, not implemented) |
| 06h/38h | Get Channel Authentication Capabilities | 1.5:18.12 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/39h | Get Session Challenge | 1.5:18.14 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/3Ah | Activate Session | 1.5:18.15 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/3Bh | Set Session Privilege Level | 1.5:18.16 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/3Ch | Close Session | 1.5:18.17 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/3Dh | Get Session Info | 1.5:18.18 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/3Fh | Get AuthCode | 1.5:18.19 | O | O | O | M | M | M | C1h (optional, not implemented) |
| 06h/40h | Set Channel Access | 1.5:18.20 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/41h | Get Channel Access | 1.5:18.21 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/42h | Get Channel Info | 1.5:18.18 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/43h | Set User Access | 1.5:18.23 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/44h | Get User Access | 1.5:18.24 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/45h | Set User Name | 1.5:18.25 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/46h | Get User Name | 1.5:18.26 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 06h/47h | Set User Password | 1.5:18.27 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| | **Chassis Device Commands** | | | | | | | | |
| 00h/00h | Get Chassis Capabilities | 1.5:22.1 | O | O | O | O | M[4] | M[4] | C1h (optional, not implemented) |
| 00h/01h | Get Chassis Status | 1.5:22.2 | O | O | O | O | M[4] | M[4] | C1h (optional, not implemented) |
| 00h/02h | Chassis Control | 1.5:22.3 | O | O | O | O | M[4] | M[4] | C1h (optional, not implemented) |
| 00h/03h | Chassis Reset | 1.5:22.4 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 00h/04h | Chassis Identify | 1.5:22.5 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 00h/05h | Set Chassis Capabilities | 1.5:22.6 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 00h/06h | Set Power Restore Policy | 1.5:22.7 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 00h/07h | Get System Reset Cause | 1.5:22.9 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 00h/08h | Set System Boot Options | 1.5:22.10 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 00h/09h | Get System Boot Options | 1.5:22.11 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 00h/0Fh | Get POH Counter | 1.5:22.12 | O | O | O | O | O | O | C1h (optional, not implemented) |
| | **Event Commands** | | | | | | | | |
| 04h/00h | Set Event Receiver | 1.5:23.1 | NA | M | M | O | M | M | C1h (NA for Tier 1) |
| 04h/01h | Get Event Receiver | 1.5:23.2 | NA | M | M | O | M | M | C1h (NA for Tier 1) |
| 04h/02h | Platform Event (a.k.a "Event Message") | 1.5:23.3 | NA | M | M | O | M | M | C1h (NA for Tier 1) |
| | **PEF and Alerting Commands** | | | | | | | | |
| 04h/10h | Get PEF Capabilities | 1.5:24.1 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/11h | Arm PEF Postpone Timer | 1.5:24.2 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/12h | Set PEF Configuration Parameters | 1.5:24.3 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/13h | Get PEF Configuration Parameters | 1.5:24.4 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/14h | Set Last Processed Event ID | 1.5:24.5 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/15h | Get Last Processed Event ID | 1.5:24.6 | NA | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (NA for Tier 1) |
| 04h/16h | Alert Immediate | 1.5:24.7 | NA | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (NA for Tier 1) |
| 04h/17h | PET Acknowledge | 1.5:24.8 | NA | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (NA for Tier 1) |
| | **Sensor Device Commands** | | | | | | | | |
| 04h/20h | Get Device SDR Info | 1.5:29.2 | M | M | M | M | M | M | implemented (se::get_device_sdr_info; Get Device SDR Info; IPMI 2.0 35.2) |
| 04h/21h | Get Device SDR | 1.5:29.3 | M[2] | M[2] | M[2] | M[2] | M[2] | M[2] | implemented (se::get_device_sdr; Get Device SDR; IPMI 2.0 35.3) |
| 04h/22h | Reserve Device SDR Repository | 1.5:29.4 | M[2] | M[2] | M[2] | M[2] | M[2] | M[2] | implemented (se::reserve_device_sdr_repository; Reserve Device SDR Repository; IPMI 2.0 35.4) |
| 04h/23h | Get Sensor Reading Factors | 1.5:29.5 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 04h/24h | Set Sensor Hysteresis | 1.5:29.6 | O | O | O | O | O | M | C1h (optional, not implemented) |
| 04h/25h | Get Sensor Hysteresis | 1.5:29.7 | O | O | O | O | O | M | C1h (optional, not implemented) |
| 04h/26h | Set Sensor Threshold | 1.5:29.8 | O | O | O | O | O | M | C1h (optional, not implemented) |
| 04h/27h | Get Sensor Threshold | 1.5:29.9 | O[2] | O[2] | O[2] | O[2] | O[2] | M | C1h (optional, not implemented) |
| 04h/28h | Set Sensor Event Enable | 1.5:29.10 | NA | O | M | O | O | M | C1h (NA for Tier 1) |
| 04h/29h | Get Sensor Event Enable | 1.5:29.11 | NA | O[2] | M | O[2] | O[2] | M | C1h (NA for Tier 1) |
| 04h/2Ah | Re-arm Sensor Events | 1.5:29.12 | NA | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (NA for Tier 1) |
| 04h/2Bh | Get Sensor Event Status | 1.5:29.13 | NA | O | O | O | O | O | C1h (NA for Tier 1) |
| 04h/2Dh | Get Sensor Reading | 1.5:29.14 | M | M | M | M | M | M | implemented (se::get_sensor_reading; Get Sensor Reading; IPMI 2.0 35.14) |
| 04h/2Eh | Set Sensor Type | 1.5:29.15 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 04h/2Fh | Get Sensor Type | 1.5:29.16 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 04h/30h | Set Sensor Reading | 2.0:35.17 | O | O | O | O | O | O | C1h (optional, not implemented) |
| | **FRU Device Commands** | | | | | | | | |
| 0Ah/10h | Get FRU Inventory Area Info | 1.5:28.1 | M | M | M | M | M | M | implemented (storage::get_fru_inventory_area_info; Get FRU Inventory Area Info; IPMI 2.0 34.1) |
| 0Ah/11h | Read FRU Data | 1.5:28.2 | M | M | M | M | M | M | implemented (storage::read_fru_data; Read FRU Data; IPMI 2.0 34.2) |
| 0Ah/12h | Write FRU Data | 1.5:28.3 | M | M | M | M | M | M | implemented (storage::write_fru_data; Write FRU Data; IPMI 2.0 34.3) |
| | **SDR Device Commands** | | | | | | | | |
| 0Ah/20h | Get SDR Repository Info | 1.5:27.9 | O | O | O | M[5] | M[5] | M[5] | C1h (optional, not implemented) |
| 0Ah/21h | Get SDR Repository Allocation Info | 1.5:27.10 | O | O | O | O[5] | O[5] | O[5] | C1h (optional, not implemented) |
| 0Ah/22h | Reserve SDR Repository | 1.5:27.11 | O | O | O | M[5] | M[5] | M[5] | C1h (optional, not implemented) |
| 0Ah/23h | Get SDR | 1.5:27.12 | O[2] | O[2] | O[2] | M[2,5] | M[2,5] | M[2,5] | C1h (optional, not implemented) |
| 0Ah/24h | Add SDR | 1.5:27.13 | O[2] | O[2] | O[2] | M[2,5] | M[2,5] | M[2,5] | C1h (optional, not implemented) |
| 0Ah/25h | Partial Add SDR | 1.5:27.14 | O[2] | O[2] | O[2] | M[2,5] | M[2,5] | M[2,5] | C1h (optional, not implemented) |
| 0Ah/26h | Delete SDR | 1.5:27.15 | O[2] | O[2] | O[2] | O[2,5] | O[2,5] | O[2,5] | C1h (optional, not implemented) |
| 0Ah/27h | Clear SDR Repository | 1.5:27.16 | O[2] | O[2] | O[2] | M[2,5] | M[2,5] | M[2,5] | C1h (optional, not implemented) |
| 0Ah/28h | Get SDR Repository Time | 1.5:27.17 | O[2] | O[2] | O[2] | O[2,5] | O[2,5] | O[2,5] | C1h (optional, not implemented) |
| 0Ah/29h | Set SDR Repository Time | 1.5:27.18 | O[2] | O[2] | O[2] | O[2,5] | O[2,5] | O[2,5] | C1h (optional, not implemented) |
| 0Ah/2Ah | Enter SDR Repository Update Mode | 1.5:27.19 | O[2] | O[2] | O[2] | O[2,5] | O[2,5] | O[2,5] | C1h (optional, not implemented) |
| 0Ah/2Bh | Exit SDR Repository Update Mode | 1.5:27.20 | O[2] | O[2] | O[2] | M[2,5] | M[2,5] | M[2,5] | C1h (optional, not implemented) |
| 0Ah/2Ch | Run Initialization Agent | 1.5:27.21 | O[2] | O[2] | O[2] | O[2,5] | O[2,5] | O[2,5] | C1h (optional, not implemented) |
| | **SEL Device Commands** | | | | | | | | |
| 0Ah/40h | Get SEL Info | 1.5:25.2 | O | O | M | O | M | M | C1h (optional, not implemented) |
| 0Ah/41h | Get SEL Allocation Info | 1.5:25.3 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 0Ah/42h | Reserve SEL | 1.5:25.4 | O[2] | O[2] | M | O[2] | O[2] | M | C1h (optional, not implemented) |
| 0Ah/43h | Get SEL Entry | 1.5:25.5 | O | O | M | O | M | M | C1h (optional, not implemented) |
| 0Ah/44h | Add SEL Entry | 1.5:25.6 | O[2] | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 0Ah/45h | Partial Add SEL Entry | 1.5:25.7 | O[2] | O[2] | O[2] | O[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 0Ah/46h | Delete SEL Entry | 1.5:25.8 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 0Ah/47h | Clear SEL | 1.5:25.9 | O | O | M | O | M | M | C1h (optional, not implemented) |
| 0Ah/48h | Get SEL Time | 1.5:25.10 | O | O | M | O | M | M | C1h (optional, not implemented) |
| 0Ah/49h | Set SEL Time | 1.5:25.11 | O | O | M | O | M | M | C1h (optional, not implemented) |
| 0Ah/5Ah | Get Auxiliary Log Status | 1.5:25.12 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 0Ah/5Bh | Set Auxiliary Log Status | 1.5:25.13 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| | **LAN Device Commands** | | | | | | | | |
| 0Ch/01h | Set LAN Configuration Parameters | 1.5:19.1 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 0Ch/02h | Get LAN Configuration Parameters | 1.5:19.2 | O[2] | O[2] | O[2] | M[2] | M[2] | M[2] | C1h (optional, not implemented) |
| 0Ch/03h | Suspend BMC ARPs | 1.5:19.3 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/04h | Get IP/UDP/RMCP Statistics | 1.5:19.4 | O | O | O | O | O | O | C1h (optional, not implemented) |
| | **Serial/Modem Device Commands** | | | | | | | | |
| 0Ch/10h | Set Serial/Modem Configuration | 1.5:20.1 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/11h | Get Serial Modem Configuration | 1.5:20.2 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/12h | Set Serial/Modem Mux | 1.5:20.3 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/13h | Get TAP Response Codes | 1.5:20.4 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/14h | Set PPP UDP Proxy Transmit Data | 1.5:20.5 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/15h | Get PPP UDP Proxy Transmit Data | 1.5:20.6 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/16h | Send PPP UDP Proxy Packet | 1.5:20.7 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/17h | Get PPP UDP Proxy Packet | 1.5:20.8 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/18h | Serial/Modem Connection Active | 1.5:20.9 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/19h | Callback | 1.5:20.10 | O | O | O | O | O | O | C1h (optional, not implemented) |
| 0Ch/1Ah | Set User Callback Options | 1.5:20.11 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 0Ch/1Bh | Get User Callback Options | 1.5:20.12 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| | **Firmware Firewall Commands** | | | | | | | | |
| 06h/09h | Get NetFn Support | 2.0:21.2 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/64h | Get OEM NetFn IANA Support | 2.0:21.11 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/0Ah | Get Command Support | 2.0:21.3 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/0Bh | Get Command Sub-function Support | 2.0:21.4 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/0Ch | Get Configurable Commands | 2.0:21.5 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/0Dh | Get Configurable Command Sub-Functions | 2.0:21.6 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/61h | Get Command Enables | 2.0:21.8 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/63h | Get Configurable Command Sub-function Enables | 2.0:21.10 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/60h | Set Command Enables | 2.0:21.7 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |
| 06h/62h | Set Configurable Command Sub-function Enables | 2.0:21.9 | O[2] | O[2] | O[2] | O[2] | O[2] | O[2] | C1h (optional, not implemented) |

<!-- end generated -->

## 3. VITA 46.11 VSO commands (Table 10.1.2-1), NetFn 2Ch, VSO 03h

"Min. privilege" is from Table 10.1.2-2; the System IPMB is session-less
and does not enforce it (p.176, p.180).

<!-- generated: sync_ipmi_commands.py -->
| Cmd | Command | Defined in | Page | IPMC T1 | IPMC T2 | IPMC T3 | ChMC T1 | ChMC T2 | ChMC T3 | Min. privilege | This IPMC (Tier 1) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 00h | Get VSO Capabilities | &sect;10.1.3.1 | 182 | M | M | M | M | M | M | User | implemented (vita::get_vso_capabilities; Get VSO Capabilities; VITA 46.11 10.1.3.1) |
| 01h | Get Chassis Address Table Info | &sect;10.1.3.2 | 184 | NA | NA | NA | M | M | M | User | C1h (NA for Tier 1) |
| 02h | Get Chassis Identifier | &sect;10.1.3.4 | 187 | NA | NA | NA | M | M | M | User | C1h (NA for Tier 1) |
| 03h | Set Chassis Identifier | &sect;10.1.3.5 | 188 | NA | NA | NA | M | M | M | Operator | C1h (NA for Tier 1) |
| 04h | FRU Control | &sect;10.1.3.6 | 189 | O | O | O/M[6] | O | M | M | Operator | C1h (optional, not implemented) |
| 05h | Get FRU LED Properties | &sect;10.1.3.27 | 209 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 06h | Get LED Color Capabilities | &sect;10.1.3.28 | 210 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 07h | Set FRU LED State | &sect;10.1.3.30 | 213 | O | O | O | O | O | O | Operator | C1h (optional, not implemented) |
| 08h | Get FRU LED State | &sect;10.1.3.29 | 211 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 09h | Set IPMB State | &sect;10.1.3.7 | 189 | M | M | M | M | M | M | Operator | implemented (vita::set_ipmb_state; Set IPMB State; VITA 46.11 10.1.3.7) |
| 0Ah | Set FRU State Policy Bits | &sect;10.1.3.9 | 192 | O/M[2] | O/M[2] | M | O | M | M | Administrator | C1h (optional, not implemented) |
| 0Bh | Get FRU State Policy Bits | &sect;10.1.3.8 | 191 | O/M[2] | O/M[2] | M | O | M | M | User | C1h (optional, not implemented) |
| 0Ch | Set FRU Activation | &sect;10.1.3.10 | 193 | O | O | M | O | M | M | Operator | C1h (optional, not implemented) |
| 0Dh | Get Device Locator Record ID | &sect;10.1.3.11 | 194 | M | M | M | M | M | M | User | implemented (vita::get_device_locator_record_id; Get Device Locator Record ID; VITA 46.11 10.1.3.11) |
| 0Eh-13h | (reserved) | | | | | | | | | | |
| 14h | Get Fan Speed Properties | &sect;10.1.3.12 | 195 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 15h | Set Fan Level | &sect;10.1.3.14 | 197 | O | O | O | O | O | O | Operator | C1h (optional, not implemented) |
| 16h | Get Fan Level | &sect;10.1.3.13 | 196 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 17h | (reserved) | | | | | | | | | | |
| 18h | Get IPMB Link Info | &sect;10.1.3.17 | 200 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 19h-1Ah | (reserved) | | | | | | | | | | |
| 1Bh | Get Chassis Manager IPMB Address | &sect;10.1.3.21 | 204 | NA | NA | NA | M | M | M | User | C1h (NA for Tier 1) |
| 1Ch | Set Fan Policy | &sect;10.1.3.15 | 198 | NA | NA | NA | O | M | M | Operator | C1h (NA for Tier 1) |
| 1Dh | Get Fan Policy | &sect;10.1.3.16 | 199 | NA | NA | NA | O | M | M | User | C1h (NA for Tier 1) |
| 1Eh | FRU Control Capabilities | &sect;10.1.3.18 | 201 | O/M[3] | M | M | O/M[3] | M | M | User | C1h (optional, not implemented) |
| 1Fh | FRU Inventory Device Lock Control | &sect;10.1.3.19 | 202 | NA | NA | NA | M | M | M | Operator | C1h (NA for Tier 1) |
| 20h | FRU Inventory Device Write | &sect;10.1.3.20 | 203 | NA | NA | NA | M | M | M | Operator | C1h (NA for Tier 1) |
| 21h | Get Chassis Manager IP Addresses | &sect;10.1.3.22 | 204 | NA | NA | NA | M | M | M | User | C1h (NA for Tier 1) |
| 22h-3Fh | (reserved) | | | | | | | | | | |
| 40h | Get FRU Address Info | &sect;10.1.3.3 | 186 | M | M | M | M | M | M | User | implemented (vita::get_fru_address_info; Get FRU Address Info; VITA 46.11 10.1.3.3) |
| 41h | Get FRU Persistent Control | &sect;10.1.3.23 | 206 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 42h | Set FRU Persistent Control | &sect;10.1.3.24 | 206 | O | O | O | O | O | O | Operator | C1h (optional, not implemented) |
| 43h | FRU Persistent Control Capabilities | &sect;10.1.3.25 | 207 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 44h | Get Mandatory Sensor Numbers | &sect;10.1.3.26 | 207 | O | O/M[4] | M | O/M[4] | O/M[4] | M | User | C1h (optional, not implemented) |
| 45h | Get FRU Hash | &sect;10.1.3.31 | 215 | O | O | M | O | O | M | User | C1h (optional, not implemented) |
| 46h | Get Payload Mode Capabilities | &sect;10.1.3.32 | 215 | O | O | M | O | O | M | User | C1h (optional, not implemented) |
| 47h | Set Payload Mode | &sect;10.1.3.33 | 216 | O | O | M | O | O | M | Operator | C1h (optional, not implemented) |
| 48h | Get Write Protect Capabilities | &sect;10.1.3.34 | 217 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 49h | Get Write Protect Enables | &sect;10.1.3.35 | 218 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 4Ah | Set Write Protect Enables | &sect;10.1.3.36 | 220 | O | O | O | O | O | O | Operator | C1h (optional, not implemented) |
| 4Bh | Get Power Supply Capabilities | &sect;10.1.3.37 | 222 | O | O | O/M[5] | O | O | M | User | C1h (optional, not implemented) |
| 4Ch | Get Power Supply Status | &sect;10.1.3.38 | 223 | O | O | O/M[5] | O | O | M | User | C1h (optional, not implemented) |
| 4Dh | Set Power Supply Status | &sect;10.1.3.39 | 224 | O | O | O/M[5] | O | O | M | Operator | C1h (optional, not implemented) |
| 4Eh | Get Control Bits Capabilities | &sect;10.1.3.40 | 225 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 4Fh | Get Control Bits | &sect;10.1.3.41 | 226 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 50h | Set Control Bits | &sect;10.1.3.42 | 227 | O | O | O | O | O | O | Operator | C1h (optional, not implemented) |
| | **Bridged Firewall Commands** | | | | | | | | | | |
| 51h | Get Bridged NetFn Support | &sect;10.1.3.47 | 233 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 52h | Get Bridged Command Enables | &sect;10.1.3.43 | 228 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 53h | Set Bridged Command Enables | &sect;10.1.3.44 | 229 | O | O | O | O | O | O | Administrator | C1h (optional, not implemented) |
| 54h | Get Bridged Command Sub-function Enables | &sect;10.1.3.45 | 230 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| 55h | Set Bridged Command Sub-function Enables | &sect;10.1.3.46 | 231 | O | O | O | O | O | O | Administrator | C1h (optional, not implemented) |
| 56h | Set Bridged NetFn Policy | &sect;10.1.3.48 | 234 | O | O | O | O | O | O | Administrator | C1h (optional, not implemented) |
| 57h | Get Bridged NetFn Policy | &sect;10.1.3.49 | 235 | O | O | O | O | O | O | User | C1h (optional, not implemented) |
| | **Notes 1. This table is leveraged and adapted from parts of [PICMG3D0] Table 3-100. 2. Mandatory if the "Set FRU Activation" command is supported. 3. Mandatory if the "Set FRU Activation" or the "FRU Control" command is supported. 4. Mandatory for IPMCs exposing more than one FRU. 5. Mandatory for IPMCs that represent a Power Supply. 6. Mandatory for Tier-3 IPMCs that represent resettable payloads.** | | | | | | | | | | |

<!-- end generated -->

## 4. Footnotes of the source tables

Table 10.1.1-1 (IPMI commands), p.176:

1. The table is adapted from IPMI v1.5 Table G-1.2.
2. Additional constraints apply; see the IPMI specification section.
3. Sent using the Broadcast format on IPMB; see the command description.
4. The Chassis Manager applies these commands to the entire chassis.
5. VITA 46.11 does not require a centralized SDR Repository in the Chassis
   Manager, though a partial SDR Repository is necessary there.
6. Supported only on a local Payload Interface (System Interface).
7. Mandatory for Tier-3 IPMCs that include a Payload Interface.
8. Mandatory for Tier-3 IPMCs with private I2C interfaces to subsidiary
   FRU EEPROMs.

Table 10.1.2-1 (VSO commands), p.180:

1. The table is adapted from parts of PICMG 3.0 Table 3-100.
2. Mandatory if Set FRU Activation is supported.
3. Mandatory if Set FRU Activation or FRU Control is supported.
4. Mandatory for IPMCs exposing more than one FRU.
5. Mandatory for IPMCs that represent a Power Supply.
6. Mandatory for Tier-3 IPMCs that represent resettable payloads.

Owner decision 10-23-2026: mandatory items only, no optional items yet.
FRU Control Capabilities (1Eh) is mandatory only when Set FRU Activation or
FRU Control is supported (footnote [3]); this IPMC supports neither, so it
returns C1h. Get Mandatory Sensor Numbers (44h) is O for a Tier-1 IPMC;
Rule 6.2-2 (p.123) concerns reporting a FRU's sensors through it, but for
FRU 0, the only FRU here, the numbers are fixed by Table 10.2-1 and the
Chassis Manager need not issue the command (Observation 5.2.2-4, p.111),
so it also returns C1h.

## 5. IPMI v1.5 chapter to IPMI v2.0 chapter

General IPMI knowledge, not from VITA 46.11: the VITA table cites IPMI v1.5
section numbers. IPMI v2.0 (rev 1.1) moved the command chapters; the
chapter mapping below locates each v1.5 reference in v2.0. Section numbers
inside a chapter mostly carry over, but IPMI v2.0 inserted commands in
some chapters (notably messaging), so confirm the minor number in the v2.0
table of contents.

| IPMI v1.5 chapter | Topic | IPMI v2.0 chapter |
|---|---|---|
| 17 | IPM Device "Global" commands | 20 |
| 18 | BMC device and messaging commands | 22 |
| 19 | LAN device commands | 23 |
| 20 | Serial/modem device commands | 25 |
| 21 | BMC watchdog timer commands | 27 |
| 22 | Chassis device commands | 28 |
| 23 | Event commands | 29 |
| 24 | PEF and alerting commands | 30 |
| 25 | SEL device commands | 31 |
| 27 | SDR repository commands | 33 |
| 28 | FRU inventory device commands | 34 |
| 29 | Sensor device commands | 35 |
