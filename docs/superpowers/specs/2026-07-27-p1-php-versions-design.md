<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# PHP Version Management — Design

**Date:** 2026-07-27
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 — makes per-site PHP version selection mean something

## 1. Goal

Let a developer have several PHP versions on the machine at once, install another from
inside OpenVHost, and point each site at the one it needs.

Per-site PHP is the product's headline feature (plan §4) and today it is honest but
useless: the site editor offers a version field, and Apply blocks with
`site X needs PHP 8.3, which is not installed (installed: 8.5)` because exactly one PHP
exists. This slice makes the other side of that sentence reachable.

Success criterion — a developer can do this on a clean machine:

1. Open **Languages**, see that only PHP 8.5 is installed.
2. Press **Install** on 8.3 and watch the log until it finishes.
3. Set a site's PHP version to 8.3, Apply, and have that site served by 8.3 while another
   site is still served by 8.5.

## 2. Starting position: the pipe exists, the supply does not

`openvhost-pkg` already does download → SHA-256 verify → extract → install into
`packages/<name>/<major>/<version>/` with a per-major `current` link (P0-6). Its own
`lib.rs` says the manifest layer — "which versions exist, at which URL" — is a separate
future slice.

The blocker is what a manifest could point at. The plan requires two things that do not
currently meet:

> "manifests must point at **official upstream downloads** so we are not the distributor"
> "own the **Phase 2+** package-build pipeline (reproducible builds of PHP/nginx/etc.)"

The official upstream distribution of PHP and nginx for macOS is **source only**. The
`php.net/distributions/php-8.4.23.tar.gz` that P0-6's live test installs is a source
tarball — it proves the pipeline, not a usable runtime. Turning it into a working
`php-fpm` means compiling, which needs a toolchain, a dependency set and tens of minutes.
That is the Phase 2 build-pipeline work, not a Phase 1 button.

Homebrew already solves the supply problem for macOS: `php@8.1` through `php@8.5` are
formulae that install side by side at `/opt/homebrew/opt/php@<major>/sbin/php-fpm`. The
owner chose to build on that now rather than wait for our own builds (decision, §9).

**This slice therefore ships PHP version management, not a package manager.** MySQL,
MariaDB and nginx are out of scope (§11).

### 2.1 Homebrew is a bridge, and we should know what removes it

ServBay — the product this plan takes reference notes from (plan §8) — does not use Homebrew
at all. It builds and hosts its own packages and downloads them at runtime, which is why a
machine with nothing installed works out of the box. It only points users at Homebrew for
advanced cases like compiling a custom PHP module. That is the model the plan already
targets: *"Service binaries are downloaded at runtime, never bundled in the installer"*
(plan §1), with bundling explicitly rejected in §8.

So depending on Homebrew is a **deliberate temporary bridge**, not the destination. What
removes it is the plan's own Phase 2 item: *"Own the Phase 2+ package-build pipeline design
(reproducible builds of PHP/nginx/etc. for our 3–4 targets) — start a docs/build-pipeline.md
ADR before implementing."*

**One correction to how that item is usually read.** The plan's licensing constraint is
scoped to *GPL* packages: *"from the moment we distribute OUR OWN builds of GPL packages
(MySQL/MariaDB etc.)"*. PHP is under the PHP License 3.01 and nginx under BSD 2-clause —
both permissive, neither carrying a copyleft source-offer duty. **Self-hosting our own PHP
build is therefore an engineering problem, not a licensing one**, which makes PHP the right
place to start and MySQL/MariaDB the part that genuinely waits for legal review (plan §7 Q6).

The engineering is still substantial and should not be understated: PHP needs roughly thirty
dependencies, and unless those are vendored too the resulting `php-fpm` links against
Homebrew's dylibs and nothing has been gained. Relocatability (`@rpath`, `install_name_tool`),
macOS quarantine attributes on downloaded binaries, and signing (plan §7 Q5, unresolved) are
all part of it. Weeks, not days — hence the ADR before the code.

The seam is already in the right place: everything downstream sees only
`PhpRuntime { major, fpm_bin }`, and the only function that knows Homebrew exists is
`discover_php_in`. Swapping the source later means adding a sibling that scans `packages/`,
which `openvhost-pkg` already installs into.

## 3. Architecture

Three pieces with one responsibility each, so that "run a program" and "know about
Homebrew" never live in the same module:

| Piece | Crate | Responsibility | Knows about brew |
|---|---|---|---|
| `openvhost-proc::task` | proc | Run one command to completion, streaming its output | no |
| `openvhost-core::php` | core | Discover installed versions; build the install `TaskSpec` | yes |
| Languages page + IPC | desktop | Join the two, then register the new service row | no |

`openvhost-core::php` **executes nothing** — it reads the filesystem and constructs a
spec. Spawning belongs to `openvhost-proc`, which keeps core `tauri`-free and lets the
`openvhost` CLI reuse both halves unchanged.

### 3.1 The task runner

```rust
pub struct TaskSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

pub enum Stream { Stdout, Stderr }

pub enum TaskEvent {
    Line { stream: Stream, text: String },
    Finished { code: Option<i32> },
}

/// Resolves when the process has exited and every line has been sent.
/// `Ok(None)` means it was killed by a signal rather than exiting normally.
pub async fn run_task(
    spec: TaskSpec,
    tx: tokio::sync::mpsc::Sender<TaskEvent>,
) -> Result<Option<i32>, ProcError>;
```

`ProcError` is reserved for failures to *run* the program at all — a missing binary, a
spawn refusal. A program that runs and exits non-zero is `Ok`, because "brew said no" is an
outcome the caller must render, not an error in the runner.

A one-shot task is a different shape from a supervised service: it has no state machine,
no restart, and reaching `Stopped` is success rather than something to report. Modelling
it as a `Supervisor` entry would make a clean `exit 0` render as `Stopped` or `Failed` in
the Services panel, which is worse than useless.

What it inherits from the supervisor's discipline is the part that matters: **its own
process group, killed as a group on drop.** `brew install` forks a tree — curl, tar, ruby,
sometimes a compiler — and abandoning that tree when the app quits mid-install is exactly
the orphan problem P0-8 closed.

**No timeout.** A twenty-minute compile is normal here, so a clock would only ever fire on
a legitimate install. Cancellation is expressed by dropping the future, which kills the
group.

### 3.2 Discovery

Walk `/opt/homebrew/opt/php@*` and `/opt/homebrew/opt/php`, then the same under
`/usr/local` for Intel machines. A candidate counts when `sbin/php-fpm` exists under it;
its major comes from `probe_php_fpm_version` (added in the site-apply slice).

The result is a `Vec<PhpRuntime>` — the type the apply pipeline already defines and
consumes. Discovery **replaces how that vector is produced**, not the vector itself, so
`render_set`, the pool generation and the `MissingRuntime` check are untouched.

**Deduplicate by major.** On the machine this was designed on, `/opt/homebrew/opt/php` and
`/opt/homebrew/opt/php@8.5` both resolve to `../Cellar/php/8.5.8` — the unversioned
formula is simply an alias for the current one. Without dedup that is two entries for one
runtime, two service rows and two pools listening on two sockets for the same binary.

**Nothing is resolved through `PATH`** — neither `php-fpm` nor `brew`. The existing probe
code already carries this rule and its reason: a ServBay install shadows `nginx` and
`php-fpm` on `PATH`. `brew` deserves the same treatment for the same reason.

### 3.3 Debt to settle on the way through

`find_brew_binaries` currently exists **twice**, in `openvhost-conf::validate` and in
`openvhost-core::platform::macos::demo_stack`. This slice rewrites discovery anyway, so it
collapses the two into one source rather than adding a third.

## 4. Installing

### 4.1 Two layers of input guarding

The newtypes in this project are charset guards, not policy — a lesson carried forward
explicitly from the site-apply slice, which had to add its own confinement on top. This is
where the policy layer belongs:

```rust
PhpMajor::parse("8.3")                        // layer 1: shape — ^\d+\.\d+$
const CATALOGUE: [&str; 5] = ["8.1", "8.2", "8.3", "8.4", "8.5"];   // layer 2: policy
let formula = format!("php@{major}");         // composed here, never taken from the webview
```

Arguments are passed as an argv vector, never through a shell. That prevents *command*
injection but not *flag* injection: without the catalogue check, a value like
`--build-from-source`, `--HEAD`, or the name of an unrelated formula flows straight into
`brew install`. Layer 2 is what closes that, and it is the reason the IPC command accepts a
version rather than a formula.

The catalogue is a hand-maintained constant. It will age as PHP releases, and that is
accepted for this slice: the alternative — asking `brew` what exists — spawns a process on
a path that must stay cheap, and a stale entry fails loudly at install time rather than
silently.

### 4.2 Locating and invoking brew

`/opt/homebrew/bin/brew`, then `/usr/local/bin/brew`. Absent from both → a message naming
where it looked.

The only environment variable set is `HOMEBREW_NO_AUTO_UPDATE=1`, so that pressing Install
does not silently spend five minutes updating Homebrew itself before starting the work the
user asked for. Nothing else in the user's environment is touched.

**One install at a time**, guarded by a mutex in managed state — the same shape the apply
pipeline uses. A version already installed offers no button.

### 4.3 After a successful install

Rescan, then `Supervisor::register` a `php-fpm-<major>` row for each newly found major.
Registration at runtime is supported: `register` takes `&self` and refuses to replace an
entry that is currently live.

What follows is emergent rather than written: `render_set` emits one pool per **installed**
major regardless of whether any site uses it (pinned by the existing test
`pools_are_rendered_for_installed_majors_nobody_uses`), so a new version immediately makes
the Sites banner report a pending change, and Apply creates the pool. The Languages row says
so, with a link to Sites.

**One real change this forces.** `InstalledRuntimes` is currently managed state set once at
startup, and Tauri cannot replace managed state. It becomes
`RwLock<Option<InstalledRuntimes>>`, and every existing reader — `plan_site_apply`,
`apply_sites`, the stack registration — is updated to take the read lock. Without this the
apply pipeline would never learn about a version installed after launch, and the Packages
page would appear to work while changing nothing.

Probing still happens **once per rescan, not per call**: the reason `plan_site_apply`
spawns no process is that discovery already ran, and that property must survive this
change.

## 5. Error surface

| Condition | What the user sees |
|---|---|
| No `brew` found | §6.1's guided state — not a bare error |
| Version outside the catalogue | Rejected at the IPC boundary, naming the field |
| Already installed | No button; the command refuses if called anyway |
| `brew install` exits non-zero | The log **stays on screen**, with the closing lines emphasised |
| **Exit 0 but the version is still not discovered** | Stated plainly: brew reported success, but no `php-fpm` was found for that version — with the paths that were searched |

The last row is the one worth building deliberately. "Succeeded but nothing changed" is the
silent-failure class this project has caught in every slice so far; without it a user
presses Install repeatedly with no way to see why nothing happens.

### 5.0 The dead end this slice exists to close

The failure the owner hit on a real machine, and the reason several of the items below are
not polish:

> Sites → the PHP dropdown offers 8.4 / 8.3 / 8.2 / 8.1 (a hard-coded constant) → the
> machine has only 8.5 → **every option leads to the same refusal** → Apply never succeeds →
> no config is ever written → the browser gets `ERR_CONNECTION_REFUSED`.

Three separate mistakes stacked:

1. **Offering a choice that cannot be satisfied** — the dropdown was unrelated to the machine.
2. **Defaulting to one of them** — a new site starts at `PHP_VERSIONS[0]`, so it is born broken.
3. **Reporting the dead end without an exit** — the banner states the problem and offers
   nothing to press.

Fixing 1 and 2 makes the trap unreachable. **Fixing 3 is still required**, because the machine
changes outside the app: a user can `brew uninstall php@8.3` at any time and strand a site
that was fine yesterday. Prevention cannot be complete, so recovery has to exist.

The banner therefore offers the actions that actually resolve it, not just the diagnosis:

- **Install PHP 8.4** → the Languages page, with that version's install started
- **Change hello to 8.5** → the site editor for that site, with an installed version selected

And the problem is surfaced *before* Apply: a site whose PHP version is not installed carries
a warning in its row in the Sites list, so it is visible when it is created rather than as a
surprise later.

### 5.1 A version that disappears

If the user runs `brew uninstall php@8.3` in a terminal, the service row remains and points
at a missing binary, so Start fails honestly with the missing path (the P0-3 spawn-failure
contract), and a site requesting 8.3 is blocked at Apply by `MissingRuntime`, which names
the versions that remain. Both behaviours already exist and are correct; this slice adds
nothing for it.

## 6. UI

A new **Languages** entry in the rail after Web Server — both answer "what is available to
run". The rail's existing disabled placeholders (Logs, Settings) are untouched.

Named *Languages* rather than *Packages* by owner decision (2026-07-27). It is the honest
label for what the section holds: PHP today, and other runtimes later. *Packages* would
promise a general package manager the slice deliberately does not build (§2), and would
have to be renamed the moment MySQL — a service, not a language — arrived.

`/languages` groups its rows **under a language heading — "PHP" — even though PHP is the only
one**. ServBay's equivalent page groups PHP, Node.js, Python and Go the same way; adopting the
shape now means adding a runtime later is a new group rather than a redesign.

| Row state | Contents |
|---|---|
| Installed | full version (8.3.14), the path it was found at, its **socket path (copyable)**, and **start/stop for that version's pool** |
| Not installed | an Install button |
| Installing | every button on the page disabled, live log beneath the row (reusing `LogPane.svelte`) |
| Failed | the log **kept on screen**, closing lines emphasised, `pre-wrap` |

**Nothing installed at all** is not a list of four disabled rows: it is a single centred call
to action for the group, the way ServBay presents an uninstalled Node.js. That is the state a
first-time user lands in, and it should read as an invitation rather than an inventory of
things they do not have.

One catalogue entry carries a **"recommended"** marker (the newest stable). A first-time user
should not have to know how 8.1 differs from 8.5 in order to get started.

**Start/stop belongs here as well as on Services** (owner decision, 2026-07-27). The mental
model is otherwise cleaner — Languages is what is installed, Services is what is running — but
it costs the user three pages to complete one intention: install on Languages, Apply on Sites,
Start on Services. ServBay puts the control on the version row and the flow ends where it
began. The two surfaces must render from the same supervisor state, not from two copies of it.

A row that has just been installed also says a pool must be created, and links to Sites —
where the banner will already be showing the pending change.

### 6.1 When the machine has nothing — the state that matters most

A user who has never installed PHP very likely has never installed Homebrew either. Saying
"no brew found, here are the paths we searched" is a dead end one level further up: they came
to this page to solve a problem and were handed a different one.

The page degrades in three honest steps:

| Machine | The page does |
|---|---|
| brew + PHP | the normal list |
| brew, no PHP | the centred install call to action |
| **no brew** | explains that OpenVHost uses Homebrew as its PHP source on macOS, shows the install command **as copyable text**, links to brew.sh, and offers **Check again** |

**We do not run Homebrew's installer.** It is a `curl | bash` that asks for `sudo` and changes
the system broadly — a decision that belongs to the machine's owner, not to an app they opened
for the first time. It would also simply fail: the process we spawn has no tty to answer a
sudo prompt, so the user would get a more confusing error than the one we started with.

**Check again** matters more than it looks. The user will leave, install brew in a terminal,
and come back; without it they must quit and relaunch — the same staleness §4.3 fixes for
installs, applied to the dependency underneath.

### 6.2 The site editor

**`SiteDrawer`'s PHP dropdown lists only versions that are actually installed** (owner
decision, 2026-07-27). Today it offers a hard-coded `['8.4','8.3','8.2','8.1']` with no
relation to the machine — on the machine this was designed against, not one of those four
is present, so every option leads to an Apply that is refused. Installing more is the
Languages page's job, not a dropdown's.

**A new site defaults to an installed version** — the newest one — rather than to
`PHP_VERSIONS[0]`. A site that is broken before the user has touched anything is the second
of §5.0's three mistakes.

The one exception is the site's **own stored value**, which is prepended and labelled when
it is not installed. Dropping it would make the `<select>` render blank and silently
rewrite the site's PHP version to whatever the browser picked instead — the bug PR #13
fixed. Keeping it is not offering an uninstalled version; it is refusing to change data
behind the user's back.

**With no PHP installed at all**, the editor does not present an empty `<select>` and a Save
button that cannot lead anywhere. It says so and links to Languages. The Sites list carries
the same pointer, because that is the page the app opens on and therefore where a first-time
user arrives before anywhere else.

**`SiteDrawer`'s PHP dropdown lists only versions that are actually installed** (owner
decision, 2026-07-27). Today it offers a hard-coded `['8.4','8.3','8.2','8.1']` with no
relation to the machine — on the machine this was designed against, not one of those four
is present, so every option leads to an Apply that is refused. Installing more is the
Languages page's job, not a dropdown's.

The one exception is the site's **own stored value**, which is prepended and labelled when
it is not installed. Dropping it would make the `<select>` render blank and silently
rewrite the site's PHP version to whatever the browser picked instead — the bug PR #13
fixed. Keeping it is not offering an uninstalled version; it is refusing to change data
behind the user's back.

## 7. Testing

**`openvhost-proc` — the task runner**, driven by the existing `proc_testchild` binary
(`--lines`, `--interval-ms`, `--exit`, `--ignore-stop`):

- every line arrives, in order, tagged with its stream
- the exit code reaches the caller
- **dropping the task kills the whole process group** — the P0-8 assertion shape
- a child that ignores a polite stop is still killed

**`openvhost-core` — discovery**, with the probe injected as a closure and the prefixes
pointed at a temp directory, so no test depends on what the running machine has installed:

- a versioned formula is found
- **`php` and `php@8.5` resolving to the same runtime yield one entry, not two**
- ordering is stable
- a prefix that does not exist is not an error

**`openvhost-core` — the allowlist**: `--build-from-source`, `--HEAD`, `8`, `9.9` (well
formed but not offered), `8.3 --foo` and the empty string are all rejected. And the
composed argv is asserted to equal exactly `["install", "php@8.3"]` — a test that fails the
moment anyone adds a flag, which is both a security property and a
no-surprises property.

**One test against real brew**: `brew --version` through `run_task`. Fast, read-only,
mutates nothing, and proves the runner works against the actual binary rather than only
against `proc_testchild`. **No test runs a real `brew install`** — minutes long, and it
would change the machine of whoever ran the suite.

**Frontend (vitest SSR)**: all four row states render; buttons are disabled while
installing (asserted in both directions); the log renders; errors are `pre-wrap`; and the
"succeeded but not detected" state renders as real UI rather than existing only in an enum.

## 8. Security posture — stated plainly

**The `openvhost-pkg` verification pipeline is not used on this path at all.** No manifest,
no pinned URL, no SHA-256 check of ours. We hand the download and its integrity checking to
Homebrew.

This is worth stating because the repository contains a carefully audited download-verify-
extract pipeline (P0-6, full security-auditor APPROVE), and a reader could reasonably assume
those guarantees cover installing PHP. They do not. It is also one of the reasons the real
manifest layer still has to come later.

What this slice does control, and what the audit should focus on:

- the catalogue allowlist actually closing flag injection
- the argv being composed rather than accepted
- `brew` located by absolute path, never `PATH`
- the process group covering brew's whole child tree
- one install at a time

This is the largest execution surface the project has added — an IPC command that causes
the app to run an external program which itself downloads and executes third-party code —
so **security-auditor APPROVE is a merge blocker** (CLAUDE.md golden rule 2).

## 9. Decisions taken (owner, 2026-07-27)

1. **Multi-PHP over Homebrew now**, rather than waiting for our own build pipeline or
   shipping a package manager whose catalogue could only contain MySQL and MariaDB. It
   unlocks the headline feature today without making us a distributor of anything.
2. **The app runs `brew install` itself**, rather than displaying a command to copy. The
   convenience is the point of the feature; the cost is that OpenVHost now runs a program
   that changes state outside its own home directory, which §8 addresses.
3. **No uninstall button.** Homebrew's packages belong to the whole machine, not to
   OpenVHost, and removing one could break work the app cannot see. A copyable command is
   offered instead.
4. **PHP only.** nginx has one Homebrew version, so a picker adds nothing; MySQL and
   MariaDB would install fine but have no lifecycle yet (no datadir, start/stop or password
   flow), so the buttons would install something unusable.
5. **The rail section is called Languages, not Packages** — it holds runtimes, and the
   slice deliberately is not a package manager (§2). The site editor's PHP dropdown
   lists only installed versions; installing more belongs on that page.
6. **Start/stop appears on the Languages row as well as on Services** — duplicating the
   control is accepted so that install-to-running does not span three pages. Both surfaces
   read the same supervisor state.
7. **Homebrew is recorded as a temporary bridge** (§2.1), not the architecture. What removes
   it is the Phase 2 build pipeline, and the licensing objection to self-hosting applies to
   MySQL/MariaDB, not to PHP or nginx.
8. **A hand-maintained catalogue constant**, accepting that it ages, rather than spawning
   `brew` to enumerate formulae on a path that must stay cheap.

## 10. Out of scope

Uninstall · nginx, MySQL, MariaDB, Apache · the signed manifest index and the
`openvhost/manifests` repo · our own reproducible builds · upgrading an installed version ·
`brew` installation itself · Windows (no Homebrew; the Windows story is a separate
enablement phase) · the `openvhost` CLI surface for any of this.
