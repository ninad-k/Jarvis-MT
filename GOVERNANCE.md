# Governance

This document describes how Arc MT is run: who decides, how decisions are made, how to take on more responsibility, how versions are numbered, and how security fixes are released.

## Model

Arc MT is **maintainer-led** (a "benevolent dictator" model). The lead maintainer, Ninad K ([@ninad-k](https://github.com/ninad-k)), has the final decision on scope, merges, releases and security response. Input from contributors is welcome and carries weight, especially on design questions, but the lead maintainer decides when there is no consensus.

Roles:

- **Lead maintainer:** sets direction, merges changes, cuts releases, and handles security reports.
- **Maintainers:** trusted contributors with merge rights. The project currently has one maintainer.
- **Contributors:** anyone who files issues, starts discussions, reviews, or opens pull requests.

## How decisions are made

- **Day-to-day changes** are discussed in issues, pull requests and [GitHub Discussions](https://github.com/ninad-k/Jarvis-MT/discussions). A change is merged when a maintainer approves it and the checks in [CONTRIBUTING.md](CONTRIBUTING.md) pass.
- **Architecture and safety decisions** are recorded as ADRs in [docs/adr/](docs/adr/README.md). A change to a core rule in [SPEC.md](SPEC.md), to the wire protocol, to a security boundary, or to the write path into MT5 needs an accepted ADR before it is merged. The ADR is proposed in a pull request and set to "Accepted" by a maintainer when it merges.
- **Core rules** in SPEC.md are not changed by a single pull request. Changing one needs an ADR that explains why the rule should change, and a maintainer who agrees.
- **Scope** is decided by the maintainers. Ideas are welcome in Discussions, but a feature needs a maintainer's agreement before significant work starts.

## Becoming a maintainer

Maintainers are trusted with merge rights, and with the safety-critical areas of the code. We look for:

- A sustained record of useful contributions: merged pull requests, careful reviews, and good issue triage.
- Sound judgment about safety. Maintainers must understand SPEC.md and be willing to hold the core rules when a change pushes against them.
- Respect for the [Code of Conduct](CODE_OF_CONDUCT.md).

To be considered, open a discussion or ask an existing maintainer. The lead maintainer decides and grants access. Maintainers who are inactive for a long period, or who break the Code of Conduct, may step down or lose access.

## Versioning

Arc MT follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) (`MAJOR.MINOR.PATCH`).

- **While the version is 0.x**, any release may contain breaking changes. This covers the HTTP API, the configuration format, the bridge wire protocol and the EA inputs. Every breaking change is listed in [CHANGELOG.md](CHANGELOG.md).
- **From 1.0.0**, breaking changes happen only in major releases.
- The wire protocol has its own version, `protocol_version`, which the `ping` command reports. A breaking protocol change increments it, and the backend refuses a mismatch.
- Release tags use the form `vX.Y.Z`.

## Security releases

- Vulnerabilities are reported privately, through GitHub's private vulnerability reporting, as described in [SECURITY.md](SECURITY.md). Do not report them in public issues.
- The maintainer confirms the report, works on the fix in a private GitHub security advisory, adds tests, and publishes a patch release as soon as it is practical. The advisory and a `Security` entry in the changelog are published with the fix.
- Reporters are credited in the advisory unless they ask not to be named.
- While the project is 0.x, only the latest release receives security fixes. Users should upgrade to it.

## Changes to this document

Propose changes to this file in a pull request, with a short discussion of the reasons. A maintainer approves it.
