"""cm_commands.py - the commands the Chassis Manager sends and the decoders of their responses.

One table, `COMMANDS`: (NetFn, command) -> name and response decoder. Request layouts follow
IPMI v2.0 (standard commands) and ANSI/VITA 46.11-2022 section 10.1.3 (group extension 2Ch,
VSO identifier 03h). A response is `bytes` starting with the completion code. The decoders return
text for the run log; they never raise on short data (a short response is reported as such).
"""

NETFN_SENSOR = 0x04
NETFN_APP = 0x06
NETFN_STORAGE = 0x0A
NETFN_GROUP_EXTENSION = 0x2C  # VITA 46.11 request NetFn (Table 10.1.2-1); the response is 2Dh

VSO_ID = 0x03  # VITA 46.11 group-extension identifier (section 10.1.2)

CMD_SET_EVENT_RECEIVER = 0x00  # NetFn 04h (IPMI v2.0 section 29.1)
CMD_PLATFORM_EVENT = 0x02  # NetFn 04h (IPMI v2.0 section 29.3): sent to the Event Receiver
CMD_COLD_RESET = 0x02  # NetFn 06h (IPMI v2.0 section 20.2)
CMD_GET_DEVICE_ID = 0x01
CMD_GET_SELF_TEST_RESULTS = 0x04
CMD_GET_DEVICE_SDR_INFO = 0x20
CMD_GET_DEVICE_SDR = 0x21
CMD_RESERVE_DEVICE_SDR = 0x22
CMD_GET_SENSOR_READING = 0x2D
CMD_GET_FRU_AREA_INFO = 0x10
CMD_READ_FRU_DATA = 0x11
CMD_GET_VSO_CAPABILITIES = 0x00
CMD_SET_IPMB_STATE = 0x09
CMD_GET_DEVICE_LOCATOR_RECORD_ID = 0x0D  # VITA 46.11 section 10.1.3.11

# Set IPMB State bytes (section 10.1.3.7): FFh = no change; bit 0 = 1 Local Control, 0 Override;
# bits 7:1 = link identification (0 = all links).
IPMB_STATE_NO_CHANGE = 0xFF
IPMB_STATE_LOCAL_CONTROL = 0x01
IPMB_STATE_OVERRIDE = 0x00

CC_OK = 0x00
COMPLETION_CODES = {
    0x00: "ok",
    0xC0: "node busy",
    0xC1: "invalid command",
    0xC3: "timeout",
    0xC7: "request data length invalid",
    0xC9: "parameter out of range",
    0xCC: "invalid data field",
    0xD3: "destination unavailable",
    0xD5: "not valid in present state",
    0xFF: "unspecified error",
}

SELF_TEST_NO_ERROR = 0x55
SELF_TEST_CORRUPTED = 0x57
FRU_ACCESS_BY_WORDS = 0x01  # Get FRU Inventory Area Info byte 4 bit 0
FRU_WORD_BYTES = 2  # a word is 16 bits (IPMI v2.0 Table 34-3)

VSO_CAPS_MIN_LEN = 8  # cc + VSO id + six data bytes (section 10.1.3.1)
DEVICE_ID_MIN_LEN = 12  # cc + the 11 bytes up to the product ID
SDR_INFO_MIN_LEN = 3
SENSOR_READING_MIN_LEN = 4
LOCATOR_RSP_LEN = 4  # cc + VSO id + record ID (2 bytes)

# Field names used by the decoders below (section 10.1.3.1).
TIER_NAMES = {0: "Tier-1", 1: "Tier-2", 2: "Tier-3"}
LAYER_NAMES = {0: "Layer 1 IPMC", 1: "Layer 2 Chassis Manager", 2: "Layer 3 System Manager"}


def cc_text(cc: int) -> str:
    """Completion code as `NNh name`."""
    return f"{cc:02X}h {COMPLETION_CODES.get(cc, 'unknown')}"


def ipmi_version(byte: int) -> str:
    """IPMI version `major.minor` of the Get Device ID version byte: the major version in the
    low nibble, the minor in the high nibble, both BCD (IPMI v2.0 Table 20-2): 02h is 2.0."""
    return f"{byte & 0x0F}.{byte >> 4 & 0x0F}"


def decode_device_id(rsp: bytes) -> str:
    if len(rsp) < DEVICE_ID_MIN_LEN:
        return f"short response ({len(rsp)} bytes)"
    dev_id, rev, fw1, fw2, ipmi_ver = rsp[1], rsp[2], rsp[3], rsp[4], rsp[5]
    mfr = rsp[7] | rsp[8] << 8 | rsp[9] << 16
    product = rsp[10] | rsp[11] << 8
    return (f"device {dev_id:02X}h rev {rev & 0x0F} sdr={rev >> 7} fw {fw1 & 0x7F}.{fw2:02X} "
            f"ipmi {ipmi_version(ipmi_ver)} support {rsp[6]:02X}h "
            f"mfr {mfr:06X}h product {product:04X}h")


def decode_self_test(rsp: bytes) -> str:
    if len(rsp) < 3:
        return f"short response ({len(rsp)} bytes)"
    names = {SELF_TEST_NO_ERROR: "no error", SELF_TEST_CORRUPTED: "corrupted or inaccessible data"}
    return f"{rsp[1]:02X}h {names.get(rsp[1], 'other')}, detail {rsp[2]:02X}h"


def decode_vso_capabilities(rsp: bytes) -> str:
    if len(rsp) < VSO_CAPS_MIN_LEN:
        return f"short response ({len(rsp)} bytes)"
    ident, ipmb, std, rev, max_fru, fru0 = rsp[2], rsp[3], rsp[4], rsp[5], rsp[6], rsp[7]
    return (f"{LAYER_NAMES.get(ident >> 4 & 3, '?')}, {TIER_NAMES.get(ident & 3, '?')}, "
            f"ipmb interfaces {ipmb_interfaces(rsp)}, "
            f"{'400' if ipmb >> 4 & 3 == 1 else '100'} kHz, standard {std & 3} rev {rev:02X}h, "
            f"max FRU id {max_fru}, IPMC FRU id {fru0}")


def ipmb_interfaces(rsp: bytes) -> int:
    """Number of IPMB interfaces in a Get VSO Capabilities response (field 00b = 1, 01b = 2)."""
    return (rsp[3] & 3) + 1 if len(rsp) >= VSO_CAPS_MIN_LEN else 0


def fru_access_unit(rsp: bytes) -> int:
    """Bytes per unit of the FRU offset and count fields of Read FRU Data, from byte 4 bit 0 of
    the Get FRU Inventory Area Info response (IPMI v2.0 Table 34-2): 1 = by bytes, 2 = by words
    (Table 34-3: the offset and the count are then in words)."""
    return FRU_WORD_BYTES if len(rsp) >= 4 and rsp[3] & FRU_ACCESS_BY_WORDS else 1


def decode_fru_area_info(rsp: bytes) -> str:
    if len(rsp) < 4:
        return f"short response ({len(rsp)} bytes)"
    size = rsp[1] | rsp[2] << 8
    unit = "words" if fru_access_unit(rsp) == FRU_WORD_BYTES else "bytes"
    return f"{size} bytes, accessed by {unit}"


def decode_read_fru(rsp: bytes) -> str:
    return f"{rsp[1]} bytes" if len(rsp) >= 2 else f"short response ({len(rsp)} bytes)"


def decode_sdr_info(rsp: bytes) -> str:
    if len(rsp) < SDR_INFO_MIN_LEN:
        return f"short response ({len(rsp)} bytes)"
    return f"{rsp[1]} sensors, flags {rsp[2]:02X}h"


def decode_reserve(rsp: bytes) -> str:
    return f"reservation {rsp[1] | rsp[2] << 8:04X}h" if len(rsp) >= 3 else "short response"


def decode_get_sdr(rsp: bytes) -> str:
    if len(rsp) < 3:
        return f"short response ({len(rsp)} bytes)"
    return f"next record {rsp[1] | rsp[2] << 8:04X}h, {len(rsp) - 3} bytes"


def decode_sensor_reading(rsp: bytes) -> str:
    if len(rsp) < SENSOR_READING_MIN_LEN:
        return f"short response ({len(rsp)} bytes)"
    return f"reading {rsp[1]:02X}h status {rsp[2]:02X}h state {rsp[3:].hex(' ').upper()}"


def locator_record_id(rsp: bytes):
    """Record ID in a Get Device Locator Record ID response (cc, VSO id, ID LS byte, ID MS byte,
    section 10.1.3.11), or None when the response is short."""
    return rsp[2] | rsp[3] << 8 if len(rsp) >= LOCATOR_RSP_LEN else None


def decode_locator_record_id(rsp: bytes) -> str:
    record = locator_record_id(rsp)
    return f"short response ({len(rsp)} bytes)" if record is None else f"record {record:04X}h"


def decode_plain(rsp: bytes) -> str:
    return rsp[1:].hex(" ").upper() if len(rsp) > 1 else ""


COMMANDS = {
    (NETFN_SENSOR, CMD_SET_EVENT_RECEIVER): ("Set Event Receiver", decode_plain),
    (NETFN_SENSOR, CMD_PLATFORM_EVENT): ("Platform Event", decode_plain),
    (NETFN_APP, CMD_COLD_RESET): ("Cold Reset", decode_plain),
    (NETFN_APP, CMD_GET_DEVICE_ID): ("Get Device ID", decode_device_id),
    (NETFN_APP, CMD_GET_SELF_TEST_RESULTS): ("Get Self Test Results", decode_self_test),
    (NETFN_SENSOR, CMD_GET_DEVICE_SDR_INFO): ("Get Device SDR Info", decode_sdr_info),
    (NETFN_SENSOR, CMD_GET_DEVICE_SDR): ("Get Device SDR", decode_get_sdr),
    (NETFN_SENSOR, CMD_RESERVE_DEVICE_SDR): ("Reserve Device SDR Repository", decode_reserve),
    (NETFN_SENSOR, CMD_GET_SENSOR_READING): ("Get Sensor Reading", decode_sensor_reading),
    (NETFN_STORAGE, CMD_GET_FRU_AREA_INFO): ("Get FRU Inventory Area Info", decode_fru_area_info),
    (NETFN_STORAGE, CMD_READ_FRU_DATA): ("Read FRU Data", decode_read_fru),
    (NETFN_GROUP_EXTENSION, CMD_GET_VSO_CAPABILITIES): ("Get VSO Capabilities",
                                                        decode_vso_capabilities),
    (NETFN_GROUP_EXTENSION, CMD_SET_IPMB_STATE): ("Set IPMB State", decode_plain),
    (NETFN_GROUP_EXTENSION, CMD_GET_DEVICE_LOCATOR_RECORD_ID): ("Get Device Locator Record ID",
                                                                decode_locator_record_id),
}


def command_name(netfn: int, cmd: int) -> str:
    """Name of a command, or `NetFn NNh cmd NNh` when the table does not hold it."""
    entry = COMMANDS.get((netfn, cmd))
    return entry[0] if entry else f"NetFn {netfn:02X}h cmd {cmd:02X}h"


def decode_response(netfn: int, cmd: int, rsp: bytes) -> str:
    """Completion code text, then the decoded body when the code is ok."""
    if not rsp:
        return "no response"
    if rsp[0] != CC_OK:
        return cc_text(rsp[0])
    entry = COMMANDS.get((netfn, cmd))
    body = entry[1](rsp) if entry else decode_plain(rsp)
    return f"ok {body}".rstrip()
