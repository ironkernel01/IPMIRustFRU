//! ipmc-ext - the bridge between the Tier-1 IPMC and the optional Tier-2/3 extension.
//!
//! When `ipmc-defs` found the sibling folder `IPMIRustFRU_Tier23`, this crate mounts its sources
//! through a relative `#[path]` link and `Active` is the extension's state. Without the folder
//! (or with `IPMC_TIER23=0`) `Active` is `NoExtension` and the build is the Tier-1 IPMC alone.
//! The firmware and the emulator hold one `Active` and pass it to `Link::poll`.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(tier23)]
#[rustfmt::skip] // the mounted tree is formatted by its own gate; absent, rustfmt cannot open it
#[path = "../../../../IPMIRustFRU_Tier23/crates/ipmc-tier23/src/lib.rs"]
mod tier23;

/// Active -- the extension this build runs: the Tier-2/3 extension when it is built in, else
/// `NoExtension`.
#[cfg(tier23)]
pub type Active = tier23::Tier23;
/// Active -- the extension this build runs: the Tier-2/3 extension when it is built in, else
/// `NoExtension`.
#[cfg(not(tier23))]
pub type Active = ipmc_core::ext::NoExtension;

// Without the extension the IPMC reports Tier-1 (ipmc-defs generates IPMC_TIER as 1).
const _: () = assert!(
    ipmc_defs::TIER23_BUILT || ipmc_defs::IPMC_TIER == 1,
    "a Tier-1 build must report IPMC_TIER 1"
);

#[cfg(test)]
mod tests {
    use super::*;
    use ipmc_core::addr;
    use ipmc_core::dispatch::Context;
    use ipmc_core::ext::Extension;
    use ipmc_core::ipmb::{self, Header, NETFN_APP};

    /// A (netFn, cmd) no Tier-1 row and no extension row owns: Get Device ID's NetFn with an
    /// unassigned command code.
    const UNOWNED_CMD: u8 = 0x7E;

    #[test]
    fn active_default_answers_nothing_for_an_unowned_command() {
        let header = Header {
            dest: 0x82,
            netfn: NETFN_APP,
            dest_lun: 0,
            src: 0x20,
            seq: 1,
            src_lun: 0,
            cmd: UNOWNED_CMD,
        };
        let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
        let n = ipmb::encode_request(&header, &[], &mut buf).unwrap();
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut ctx = Context::new(addr::resolve(0x7F));
        let mut out = [0u8; ipmb::IPMB_RESPONSE_DATA_MAX];
        assert!(Active::default()
            .command(&mut ctx, &req, &mut out)
            .is_none());
    }

    #[test]
    fn the_bridge_and_ipmc_defs_agree_on_the_extension() {
        assert_eq!(ipmc_defs::TIER23_BUILT, cfg!(tier23));
    }
}
