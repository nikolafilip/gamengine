# Licensing

gamengine is open source from day one. The repository uses a license split
(PLAN.md section 9). Adopted 2026-09-30 as the recommended split; the director
may still change it before the first public release (PLAN.md section 12).

| Path | License | File |
|---|---|---|
| `crates/gm-core`, `gm-bsp`, `gm-net`, `gm-client`, `gm-ai`, `gm-tools`, `gm-bot` | GNU GPL v3.0 or later | `LICENSE-GPL-3.0.txt` |
| `crates/gm-server`, `crates/gm-hub` | GNU AGPL v3.0 or later | `LICENSE-AGPL-3.0.txt` |
| `assets/` (maps, textures, models, audio), `docs/` | CC BY-SA 4.0 | `LICENSE-CC-BY-SA-4.0.txt` |
| `scripts/`, `.github/`, build glue | GNU GPL v3.0 or later | `LICENSE-GPL-3.0.txt` |

Why this split: anyone may run a private server, but nobody may run a modified
server as a closed network service (AGPL section 13). The client and engine are
GPL so forks stay open. Content is CC BY-SA so it can be reused with credit and
stays shareable.

GPLv3 and AGPLv3 are compatible: the server binaries link the GPL crates, and
the combined work is distributed under the AGPL as permitted by GPLv3 section 13.

Third-party tools fetched by `scripts/fetch-ericw-tools.sh` (ericw-tools, GPLv3;
Embree, Apache-2.0; oneTBB, Apache-2.0) are not vendored in this repository.

Contributions are accepted under the license of the path they touch
(inbound = outbound). No copyright assignment.
