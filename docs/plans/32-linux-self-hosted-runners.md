# Linux runner pool

## Current sizing plan (2026-11-08)

The user approved two larger runners after the initial installation. This plan
supersedes the initial concurrency and Nix configuration decisions below.

Affected paths: `ops/github-runner@.service`, `ops/ci-runners.slice`, new
`ops/ci-nix.conf` and `ops/nix-daemon-resource-limits.conf`, this record, and task
32 in the execution checklist. On the host, update the corresponding systemd
definitions, add `/etc/nix/ci-runners.conf`, and include it from the existing
`/etc/nix/nix.custom.conf`. Back up existing files before changing them. Keep the
installer-managed `/etc/nix/nix.conf` unchanged.

Ownership and interfaces:

- Keep `linux-blixt-europe` and `linux-blixt-europe-2` enabled, each with six CPU
  workers, a six-CPU quota, a 12 GiB memory pressure threshold, and 16 GiB hard cap.
- Stop and disable `linux-blixt-europe-3`; retain its registration, account,
  installation, and caches for easy reactivation. It should appear offline.
- The shared slice covers both runner services and the Nix daemon: 12 CPUs,
  28 GiB memory pressure threshold, and 32 GiB hard cap for all CI processes.
- Nix defaults to two simultaneous derivations and three cores per derivation.
  Since concurrency settings are not a global resource boundary across clients,
  also cap the daemon and its child builds at six CPUs and 8 GiB RAM, within the
  shared slice. Preserve untrusted runner access and existing Nix settings.
- This also limits Nix package builds from other local users; project build
  commands outside Nix remain subject to their respective runner quotas.

Invariants: no active Actions job or Nix build is interrupted; check both before
restarting. No container, wallet/database volume, repository access policy, build
workflow, or published SDK change. The user authorized these service/configuration
updates; no runner registration is removed.

Validation: verify systemd syntax, effective service properties and live cgroup
files; confirm two online/idle runners and the third offline; verify Nix's effective
defaults, daemon cgroup, untrusted account access, and a real tiny fixed-output
Nix derivation using the built-in fetcher, executed as a runner account without
installing another toolchain. Preserve current container identities/start
times across the change. Record outcomes without claiming an Android benchmark.

## Engineering plan (2026-11-07)

Scope: install and register runners on the `linux-blixt-europe` host, the 18-vCPU Debian 12
host. Install runner services first; migrating Android compilation, project Nix
flakes, and a successful Actions build are separate follow-ups.

Affected paths: `ops/github-runner@.service`, `ops/ci-runners.slice`, this plan,
and task 32 in `docs/production-migration-todo.md`. On the host, add only dedicated
runner accounts/homes under `/var/lib/github-runners` and these two systemd units.
Do not change existing containers, wallet/database volumes, or Nix configuration.

### Interfaces, ownership, and concurrency

- Register three organization runners for `smolcars`: `linux-blixt-europe`,
  `linux-blixt-europe-2`, and `linux-blixt-europe-3`. They share the custom label
  `linux-blixt-europe`; omit generic default labels to avoid accidental scheduling.
- Use a dedicated organization group with selected repository access. Initially
  select no repositories: installation alone must not expose the shared host to
  public pull requests. Enable access only as part of a trusted workflow design.
- Each runner gets its own unprivileged `gha-<runner-name>` account, private home,
  installation, work directory, and caches. No sudo, Docker group, wallet mounts,
  or GitHub administrator token on the server. Nix remains an untrusted daemon
  client, using the existing multi-user installation.
- Start at four CPU workers and a 10 GiB hard memory cap per runner; the parent
  slice caps the pool at 12 CPUs and 30 GiB. At inspection, existing services used
  roughly five CPUs and 8 GiB RAM. Runner processes and child builds inherit the
  limits. Nix daemon builds are outside this slice; daemon build limits must be
  addressed before migrating actual builds.
- Install the official Linux x64 runner v2.338.2 archive, verified against GitHub's
  published SHA-256:
  `baebf204307bac6193480852732f0aba2ae97b6c5597c244f4d5558390147c40`.
  Keep upstream automatic updates enabled. The runner uses host runtime libraries;
  per-project build toolchains will come from pinned Nix environments.

### Invariants and dependencies

The existing Bitcoin/LND services, data, ports, Nix daemon, production workflows,
and published SDK artifacts remain untouched. Do not grant root-equivalent Docker
access. An organization runner token with write access is required for registration
and runner-group configuration; the existing local CLI authentication has repository
administration access but lacks organization runner permissions. Transfer only the
short-lived registration token over SSH, without logging its value.

Nix 2.35.3 (Determinate 3.24.2) is already installed. The host still lacks exposed
virtualization extensions and `/dev/kvm`; emulator tests stay on Blacksmith. Three
runners represent three concurrent jobs, not three full-host builds. Separate users
and systemd restrictions reduce access but are not disposable VM isolation.

### Validation

1. Verify archive checksum and each runner's reported version.
2. Validate the units with `systemd-analyze verify` on the actual Debian host.
3. Verify each account can contact the Nix daemon, has no privileged groups, and
   cannot read another runner's home or access the host Docker socket.
4. Enable services and confirm their active state and effective cgroup limits.
5. Verify GitHub reports three online, idle runners in the restricted group and
   that no repositories have access before the workflow migration.
6. Record actual outcomes and limits below; online registration does not count as
   a successful CI build, load benchmark, or Android acceptance run.

## Execution outcomes

### Host preparation (2026-11-08 UTC)

Implemented: all three private runner accounts and installations, official
v2.338.2 archive with matching SHA-256, and the two systemd unit definitions.
The archive ships `bin/runsvc.sh`; copy it into each installation's root, as the
upstream service installer does, before validating the unit's executable path.
Listeners are not enabled or started before GitHub registration.

Validation commands and results:

- `Runner.Listener --version`: `2.338.2` for all three installations.
- `systemd-analyze verify github-runner@linux-blixt-europe.service ci-runners.slice`:
  passed after placing `runsvc.sh` in the expected path.
- A temporary `ExecStart` override ran actual Python assertions through each
  runner's systemd unit. All three passed: non-root UID, no supplementary
  privileged groups, other runner homes unreadable, Docker socket inaccessible,
  system configuration unwritable, and Nix daemon access with `trusted: false`.
- Each restricted service successfully evaluated `builtins.toFile` through Nix
  and read the resulting store object. Daemon version: `2.35.3`.
- Actual child cgroups report `cpu.max = 400000 100000` (four CPUs) and
  `memory.max = 10737418240` (10 GiB). Temporary overrides were removed and
  systemd reloaded; only the production unit definitions remain effective.

Artifacts / device or CI run evidence: checks ran on the `linux-blixt-europe` host through
systemd and are retained in its journal under the three unit names. No Actions
job or Android build has run on these installations.

External host change observed during setup: the initial inventory listed seven
running Docker containers; a later read-only check found no containers, including
stopped containers, and RAM use fell to 1.3 GiB. The Docker daemon remained active
with its original October 18 start time and `/var/lib/docker` data root. No setup
command stopped, removed, or changed containers/volumes. Their disappearance is
not attributed to runner installation, and no recovery was attempted.

Organization authentication: the supplied credential authenticated successfully as
the organization owner, but is a classic PAT with only `manage_runners:org`.
GitHub returned HTTP 403 for organization self-hosted runner and group endpoints;
the response explicitly requires `admin:org` for classic tokens. Both API versions
`2022-11-28` and `2026-03-10` returned the same result. No group or registration was
created. The token value was not logged or sent to the server.

The replacement fine-grained PAT also authenticates as the owner, but GitHub
returns `403 Resource not accessible by personal access token` for both group
listing and runner registration. Response headers require
`organization_self_hosted_runners=read` for listing and `=write` for registration.
The token's configured resource owner, organization permissions, and approval
state must be checked in GitHub; successful user authentication alone does not
prove organization access. No registration token was issued.

### Registration and service acceptance (2026-11-08 02:48 UTC)

Implemented: the final supplied credential successfully accessed the organization
runner APIs. Created runner group `linux-blixt-europe` (ID 12), with
`visibility: selected`, no selected repositories, and public repository access
disabled. Existing runner groups and Blacksmith runners were not changed.

Registered these runners in that group, each with only the custom
`linux-blixt-europe` label:

- `linux-blixt-europe`: GitHub runner ID 145507.
- `linux-blixt-europe-2`: GitHub runner ID 145508.
- `linux-blixt-europe-3`: GitHub runner ID 145509.

The administrator credential remained on the controlling Mac. Only a short-lived
organization registration token was sent over SSH, in process input, and passed
to the configuration process using `ACTIONS_RUNNER_INPUT_TOKEN`; it was not
placed in command arguments or written to a host token file. The runner stores
its own connection credentials in its private home, as required by GitHub.

Two bootstrap details were corrected before acceptance: use the absolute
`/usr/sbin/runuser` path with the restricted child environment, and decode the
runner's generated `.runner` metadata using `utf-8-sig` because it contains a BOM.
The retries reused the existing group and first registration rather than
replacing them.

Validation commands and results:

- GitHub group runners API: exactly the three expected names, all `online`, all
  `busy: false`, each with only `linux-blixt-europe` as its label.
- GitHub group repositories API: `total_count: 0`; access remains closed until
  trusted build workflows are ready.
- `systemctl is-enabled` and `systemctl is-active`: `enabled` and `active` for
  all three `github-runner@<name>.service` units; restart count zero.
- Journals: each listener reports `Connected to GitHub`, version `2.338.2`, and
  `Listening for Jobs` at 02:48:52–53 UTC.
- Effective unit properties: each runs as its own non-root account, with four
  CPUs, 10 GiB maximum memory, `NoNewPrivileges=yes`, `ProtectSystem=strict`,
  `PrivateDevices=yes`, and no temporary overrides.
- Parent pool: 12 CPUs, 24 GiB memory pressure threshold, 30 GiB hard memory cap.
- Nix daemon remains active, and all three accounts still report
  `trusted: false`, daemon version `2.35.3`.
- `git diff --check`: passed.

Artifacts / device or CI run evidence: live organization registration at
<https://github.com/organizations/smolcars/settings/actions/runners> and service
journals on the host. No Actions job, Android build, or concurrency benchmark was
run; these checks establish runner installation and connectivity only.

Operations: `systemctl status github-runner@linux-blixt-europe.service` and
`journalctl -u github-runner@linux-blixt-europe.service` inspect the first runner;
substitute either suffixed name for the others. Unit definitions are tracked under
`ops/` and installed at `/etc/systemd/system/`. Installations and workspaces live
under `/var/lib/github-runners/<name>/runner`, with each account's home one level
above the installation.

### Two-runner sizing acceptance (2026-11-08)

Implemented: updated the tracked service/slice definitions, installed the Nix
configuration fragment and daemon drop-in, restarted the two retained runners and
the idle Nix daemon, and stopped/disabled the third runner. Its registration,
account, installation, and caches remain intact. Appended the fragment include to
`/etc/nix/nix.custom.conf`; the installer-managed `nix.conf` is unchanged.

Existing configuration was backed up at
`/var/backups/bark-ci/20261108T025740Z`. The initial validation attempt encountered
an unavailable `man nix-daemon` page from the existing daemon unit's documentation
reference. Automatic rollback restored the original file hashes and active
services. Retrying `systemd-analyze verify --man=no` checked the units without that
unrelated documentation lookup and passed.

Validation commands and results:

- Before changes, the GitHub API reported all three runners idle, and no process
  ran under a Nix build account. No active Actions job or derivation was stopped.
- Both active services are enabled and online/idle in GitHub. The third is
  `inactive`/`disabled` locally and `offline`/idle in GitHub.
- Both live runner cgroups report `cpu.max = 600000 100000` and
  `memory.max = 17179869184`. Their process environments contain
  `CARGO_BUILD_JOBS=6` and `GRADLE_OPTS=-Dorg.gradle.workers.max=6`.
- The Nix daemon is active at
  `/ci.slice/ci-runners.slice/nix-daemon.service`, with
  `cpu.max = 600000 100000` and `memory.max = 8589934592`.
- The parent CI slice reports `cpu.max = 1200000 100000` and
  `memory.max = 34359738368`. This is a shared ceiling for runners plus Nix,
  rather than an additional allowance for the daemon.
- An unprivileged runner account reports Nix defaults `max-jobs = 2`, `cores = 3`,
  and daemon access with `trusted: false`.
- Actual `nix build --offline --no-link --json --expr ...` built the tiny
  fixed-output check using `builtin:fetchurl` and a store-local source. Verified
  the output bytes at
  `/nix/store/54yci2fqmdccj3a8j118rf1bkak7jf1v-ci-runner-resource-check`.
- Container ID/name inventories immediately before and after the successful
  update matched. No Docker commands mutated containers or volumes.
- `git diff --check`: passed.

Artifacts / device or CI run evidence: actual service/cgroup inspection, live
GitHub registration state, the Nix store derivation/output, and configuration
backups on the host. No Android build or performance benchmark was run.

Remaining limits: repository access, project flakes, load benchmarking, and the
Android build/test split remain follow-ups. The shared 32 GiB ceiling includes
Nix, so the runners' individual memory ceilings are not guaranteed reservations.
Do not enable public PR execution based solely on account or Nix separation.
