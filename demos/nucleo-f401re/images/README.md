# Tier-1 image files

No verified Tier-1 images are bundled in this checkout yet. The existing local
firmware build outputs identify themselves as Tier-3 and are excluded from the
public demo. Cargo and GNU Arm binutils are unavailable in the current environment,
so a fresh Tier-1 build has not run here.

Run `demos/nucleo-f401re/package.ps1` from the repository root on a configured build
PC. After a successful build it writes:

| File | Purpose |
|---|---|
| `ipmc-board-tier1.bin` | Initial installation: combined bootloader and application |
| `ipmc-nucleo-f401re-tier1.hex` | Application update through the UART bootloader |
| `SHA256SUMS.txt` | SHA-256 digests of the loadable images |
| `manifest.json` | Build provenance, configuration digest and image hashes |

Check that the manifest and hashes accompany any image release. Historical board
test results do not validate newly built images; perform the documented board
checks before distributing a release. No Tier-2/3 images belong in this directory.
