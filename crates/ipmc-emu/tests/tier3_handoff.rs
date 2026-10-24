// tier3_handoff.rs - the emulator samples at once after it applies a payload power request, as the
// firmware does (`Sensing::serve_hand_offs`), so a command that follows sees the new power state
// and not the one of the last periodic sample. At Tier-3 this keeps a power-on in progress from
// reaching M4 with Activation-Locked set (VITA 46.11 Rules 4.1.5.4.2-1/-3): Set FRU Activation
// Activate in M1, then Set FRU State Policy Bits AL=1 before the next periodic sample.

use ipmc_core::fru_state::State;
use ipmc_core::ipmb::{self, Header, NETFN_GROUP_EXT};
use ipmc_core::queue::Bus;
use ipmc_core::sensor::PowerState;
use ipmc_core::trace::Trace;
use ipmc_emu::emulator::{EmuConfig, Emulator};

const OWN: u8 = 0x82;
const CM: u8 = 0x20;
/// VITA 46.11 group extension code and FRU Device ID 0.
const VITA: u8 = 3;
const SET_FRU_STATE_POLICY_BITS: u8 = 0x0A;
const SET_FRU_ACTIVATION: u8 = 0x0C;
const POLICY_AL: u8 = 1;
const ACTIVATE: u8 = 1;

struct Lines;

impl Trace for Lines {
    fn line(&mut self, _args: core::fmt::Arguments) {}
}

fn exchange(emu: &mut Emulator, cmd: u8, seq: u8, data: &[u8], now_ms: u32) -> u8 {
    let header = Header {
        dest: OWN,
        netfn: NETFN_GROUP_EXT,
        dest_lun: 0,
        src: CM,
        seq,
        src_lun: 0,
        cmd,
    };
    let mut frame = vec![0u8; ipmb::IPMB_MAX_LEN];
    let len = ipmb::encode_request(&header, data, &mut frame).unwrap();
    emu.rx(Bus::A, &frame[..len], now_ms).unwrap();
    assert_eq!(emu.poll(now_ms, &mut Lines), 1);
    let (bus, bytes) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    emu.tx_done(bus, true, now_ms + 1, &mut Lines).unwrap();
    let parsed = ipmb::parse(&bytes).unwrap();
    parsed.completion_code.unwrap()
}

#[test]
fn a_power_on_in_progress_does_not_reach_m4_with_al_set() {
    if ipmc_defs::IPMC_TIER < 3 || ipmc_defs::IPMC_SIM_PGOOD_HIGH != 1 {
        return;
    }
    let config = EmuConfig {
        power_on_at_start: false,
        ..EmuConfig::from_defs()
    };
    let mut emu = Emulator::new(config).expect("emulator");
    let mut now = 0u32;
    // M0 to M1 on the first samples; the payload is off.
    for _ in 0..5 {
        emu.poll(now, &mut Lines);
        now += 1;
    }
    assert_eq!(emu.context().fru_state.state(), State::M1);
    assert_eq!(emu.payload_power(), PowerState::Off);
    // Activate, then set AL in the next millisecond: before the next periodic sample.
    assert_eq!(
        exchange(&mut emu, SET_FRU_ACTIVATION, 1, &[VITA, 0, ACTIVATE], now),
        0
    );
    now += 2;
    assert_eq!(emu.context().sensors.power, PowerState::PowerOnRequested);
    let set_al = [VITA, 0, POLICY_AL, POLICY_AL];
    assert_eq!(
        exchange(&mut emu, SET_FRU_STATE_POLICY_BITS, 2, &set_al, now),
        0
    );
    // PGOOD follows PWR_EN in the emulator; let several sample periods pass.
    for _ in 0..200 {
        now += 10;
        emu.poll(now, &mut Lines);
        assert_ne!(emu.context().fru_state.state(), State::M4, "at {now} ms");
    }
    assert_eq!(emu.context().fru_state.state(), State::M1);
    assert_eq!(emu.payload_power(), PowerState::Off);
}
