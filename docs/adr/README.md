# Architecture Decision Records

Capture in-the-moment decisions with immediate pros and cons weighed.

Not prescriptive - starting points to revisit as understanding evolves. Useful for aligning humans and LLMs while the decision stands.

When a decision changes, append a new ADR that references and supersedes the previous.

- [0001](0001-directed-grants.md) Directed grants -- default deny, `(port) -> grantee`
- [0002](0002-token-enrollment.md) Token enrollment and identity persistence (names historical)
- [0003](0003-host-attested-enrollment.md) Host-attested enrollment -- the host vouches for a key the workload generated
- [0004](0004-peer-surfaces.md) Peer surfaces -- a peer's ports at their own address (partly superseded by 0005)
- [0005](0005-auto-project-and-owned-resolver.md) Auto-project and the owned `.pai-sho` resolver
- [0006](0006-invitations.md) Invitations -- `invite` / `accept`, and a grant always names its grantees
- [0007](0007-pure-core.md) A pure core for admission and authorization
