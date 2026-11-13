# IPMIRustFRU Source-Available License

Revision 11-13-2026.

Copyright (C) 2026 IronKernel. All rights reserved except as granted below.

## 1. Definitions

- **IPMIRustFRU** is the content of this repository: the Rust IPMC firmware, libraries, board
  support, bootloader, emulator, host tools, tests, scripts and documents, in source form
  and in any built form (firmware image, executable or library). It excludes the private
  Tier-2/3 extension, other separately licensed components and the private inputs under
  `local_only/`.
- **You** is the person or the organisation that uses IPMIRustFRU.
- **Product** is any hardware, firmware, software or service that contains IPMIRustFRU or a part
  of it, modified or not, and that You sell, lease, license, deploy for a customer, or use to
  provide a paid service.
- **Design Sources** are everything needed to rebuild the firmware and software of a Product,
  and any programmable-logic design that forms part of it: source code, configuration,
  board-support code, drivers, build scripts and, where applicable, RTL, constraints and
  block designs, including Your own parts of the design. Third-party parts retain their
  own licenses as stated in section 6.4.
- **Changes** are Your modifications of IPMIRustFRU files and Your new files that are derived from
  IPMIRustFRU files.
- **Paid Features** are the features that IronKernel delivers only under a paid license and
  that are not in this repository, including the private Tier-2/3 extension.

## 2. Evaluation license (no fee)

You may use, copy, modify, simulate, build and run IPMIRustFRU, without a fee, for:

- evaluation and development, including prototypes and demonstrations that are not sold;
- research and teaching;
- hobby and personal projects.

Under this section You may not ship, sell, lease or deploy a Product. A Product needs
section 3 or section 4.

## 3. Open Product license (no fee)

You may make, sell, ship, lease, license or deploy a Product, or use it to provide a paid
service, without a fee when all of these conditions hold for as long as You do so:

1. **Public Design Sources.** You publish the complete Design Sources of the Product, at no
   charge and without registration, under the terms of this license (except third-party
   parts under section 6.4), no later than the day the Product is first shipped, leased, licensed, deployed for a customer or
   used to provide a paid service. Each later release of the Product has its Design Sources
   published the same way.
2. **Changes contributed back.** You offer every Change to the IronKernel GitHub repository
   as a pull request, no later than 90 days after the Product release that contains it.
   IronKernel may accept, change or decline a pull request. The condition is met by the
   offer.
3. **Notice.** The Product's documentation names IPMIRustFRU, gives the address of this
   repository and gives the address of the published Design Sources.

If a condition stops holding, the Open Product license of that Product ends, and section 4
applies to further shipments, leases, licenses, customer deployments and paid-service use.

## 4. Commercial license (paid)

A Product whose Design Sources are not public, or that does not meet every condition of
section 3, needs a paid license from IronKernel before it is shipped, leased, licensed,
deployed for a customer or used to provide a paid service. Contact IronKernel through
ironkernel.com for the available terms and prices. A paid license
is a separate written agreement; where it differs from this text, it governs for that
Product. Paid licenses carry no royalty and no per-unit fee.

The Tier-1 commercial license fee is USD 5,000 per product/SKU. Tier-1 maintenance
has no annual fee; public updates remain available, without a promised support response
time. A different product/SKU requires its own license unless an enterprise agreement
covers it. Enterprise licensing for multiple product SKUs is available upon request
and quotation. The private Tier-2/3 extension requires its own commercial agreement.

## 5. Paid Features

The Paid Features are not licensed by this text. A demonstration form of a Paid Feature
that IronKernel distributes for evaluation (for example a time-limited firmware image)
comes with its own evaluation terms. You may not remove, bypass or extend the time limit of a
demonstration form, and You may not ship it in a Product.

## 6. Conditions for every use

1. **Notices stay.** Copies of IPMIRustFRU, modified or not, keep the copyright notices, this
   license file and the file headers.
2. **Marked changes.** A modified file states that it was changed and the date.
3. **Same license for IPMIRustFRU.** IPMIRustFRU files and Changes stay under this license when You
   pass them on in source form. You may not offer IPMIRustFRU, or a work that is mainly IPMIRustFRU,
   as a competing IPMC firmware or software product or under different terms.
4. **Third-party parts.** Components that carry another license in their headers, notices
   or license files (for example third-party Rust crates or Python packages) stay under
   that license. This text does not change it.
5. **Names.** This license grants no right to the names IronKernel and IPMIRustFRU beyond the
   notice of section 3 and a truthful statement that a design uses IPMIRustFRU.

## 7. Contributions

A contribution that You offer to the IronKernel GitHub repository is licensed by You to
IronKernel and to every user of IPMIRustFRU under this license, and You also grant IronKernel
the right to include it in versions of IPMIRustFRU that IronKernel licenses under paid terms.
You confirm that You have the right to offer it. No separate contributor agreement is
required.

## 8. Patents

Each contributor grants You a license, limited to the use this text permits, under the
contributor's patent claims that its contribution to IPMIRustFRU necessarily infringes. This
license grants no right under any third-party patent. Any third-party patent license
needed to implement a standard or to make, use or distribute a Product is Your
responsibility. This license also grants no rights to reproduce or distribute licensed
IPMI, VITA or other standards documents.

If You start a patent claim against IronKernel or a contributor that alleges IPMIRustFRU
infringes a patent, Your rights under this license end on the day the claim is filed.

## 9. No warranty

IPMIRustFRU is provided as it is, without warranty of any kind, express or implied, including
the warranties of merchantability, fitness for a particular purpose and non-infringement.
IPMIRustFRU is not designed or tested for use where a failure can cause death, injury or
serious damage to property.

## 10. Limit of liability

To the extent the law allows, IronKernel and the contributors are not liable for any
damages that arise from the use of IPMIRustFRU or from the inability to use it, including
direct, indirect, incidental, special and consequential damages and loss of data or
profit, whatever the legal basis of the claim.

## 11. Termination

Your rights under this license end when You break one of its terms and do not correct the
breach within 90 days after You learn of it. Rights that others received from You under
this license stay in force while they keep to its terms. Sections 9 and 10 stay in force
after the license ends.

## 12. General

- This license is source-available. It is not an open source license as the Open Source
  Initiative defines the term, because sections 3 and 4 restrict commercial use.
- IronKernel may publish later revisions of this license. A release of IPMIRustFRU stays under
  the revision it was published with; You may choose a later revision for it.
- This license is governed by the law of the State of Florida, without its conflict-of-law
  rules. The courts located in Florida have exclusive jurisdiction over disputes about it.
- If a court finds one term unenforceable, the other terms stay in force.
- Questions about this license and requests for a paid license go to the contact named on
  ironkernel.com.
