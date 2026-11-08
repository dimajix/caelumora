# 548 — A workspace's container has to run Open Terminal's server

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Integrations
**Summary:** A workspace of an `open_terminal_docker` storage can only run in Open Terminal's image, needs a network Caelumora reaches it through, and keeps a key and a server beside the commands it is meant to contain, although Caelumora holds the Docker daemon that could reach the container without any of them.

## The problem

An `open_terminal_docker` storage (issue [251](251-open-terminal.md), part A) starts a container per
workspace and then talks to the Open Terminal server inside it over HTTP. The daemon that started
the container could run the commands itself, by `exec`. That the server is in between costs:

- **The image is Open Terminal's.** Which tools an agent has is what that image brings or what its
  entrypoint installs at every start (`START_WAIT` is 120 s for that reason). An image built for a
  project - its compilers, its libraries - cannot be used as it is.
- **The container needs a network.** Caelumora reaches it at its address, so
  `DockerStorageConfig::check` refuses `none` (`caelumora-open-terminal/src/docker.rs:120`). A
  workspace that may not reach anything cannot be had.
- **A key and a server lie beside the commands.** A command can read the key from the server's
  process (WORKSPACE.md), and, running as the same user, can interfere with the server whose
  answers Caelumora takes for what happened.
- **A 0.x API of someone else's**, whose peculiarities `caelumora-open-terminal/src/terminal.rs`
  lists: no `stat`, offsets that count pieces of output, a character split between two pieces,
  files written in place and from memory, a removal of a directory with everything in it, so that
  emptiness is checked first and a command in between can undo that check.

None of this applies to `open_terminal_shared`: there Caelumora does not hold a daemon, and the
HTTP API is the only way in.

## Why it matters

A workspace's container is the boundary around the most dangerous tool an agent is given. No
network at all, nothing of Caelumora's in it, and an image chosen for the work are what an operator
wants of that boundary, and today none of the three can be had.

## What Docker offers, and what it does not

From the Engine API's definition (`api/swagger.yaml` of moby, version 1.57, read 2026-11-08):

- **Commands:** `POST /containers/{id}/exec`, `POST /exec/{id}/start`, `POST /exec/{id}/resize` and
  `GET /exec/{id}/json` - **nothing that ends one.** Dropping the connection does not end the
  process either; the request for an end point is moby's issue 9098, open, and its pull request
  41548 unmerged. `cmd` is an array, so that what is passed needs no quoting.
- **Output** comes only over the stream attached when the exec is started. It is neither kept nor
  readable from an offset, and is gone for whoever was not attached.
- **Files:** `/containers/{id}/archive` - `HEAD` the metadata of one path, `GET` a tar of a path,
  `PUT` a tar unpacked into a directory. `GET` on a directory gives the whole tree with its content,
  so that it is no listing; there is no removing or moving.

So files can be had; what a server inside the container really did for us is **keeping commands**:
their process group, their output, their exit code.

## Considerations

### Decision 1: what controls the container

- **A. Stay with Open Terminal.** No work, and all of the problem above.
- **B. A small static program of our own in the container**, speaking JSON over the stdin and stdout
  of an exec. Exact semantics, independent of what the image holds; but one more artifact to build
  for every architecture and to bring into the container.
- **C. Shell scripts passed by exec (chosen).** The scripts are compiled into Caelumora
  (`include_str!`) and run as `["sh", "-c", script, "sh", arg…]`, so that nothing is installed in
  the image and paths and commands arrive as arguments, never as text to be quoted. A shell becomes
  what an image has to bring, which is a condition nearly every image meets; in return the choice of
  images and their tooling is free.

Decided by the maintainer on 2026-11-08: C, for that flexibility.

A shell alone is not enough, and the condition has to be said exactly: `cat`, `mkdir`, `mv`, `rm`,
`rmdir`, `sleep`, `stat`, `tail` are programs (`kill` is built into the shell). `stat -c` is not
POSIX, but coreutils and BusyBox share it. `find -printf` is GNU's only and is not used, nor are
`timeout` and `setsid` where it can be helped. An image without them - distroless, `scratch` - is
told apart when its container is first started: a script asks `command -v` for each and the
storage fails naming what is missing.

### Decision 2: `sh` or `bash`

- **POSIX `sh` (recommended).** Every candidate image has it, Alpine's among them, which has no
  `bash` by default. Nothing the scripts need calls for more.
- **`bash` required.** More comfortable to write, and shuts out the smallest images - the
  flexibility this issue is for.

### Decision 3: beside `open_terminal_docker`, or instead of it

- **Instead (recommended).** A storage `docker` replaces `open_terminal_docker`. Two providers that
  start a container per workspace would be a variant beside the pattern. What `docker.rs` does -
  the volume, the container found by its labels, the idle stop, the slots - is kept, in a crate of
  its own (`caelumora-docker`, a plugin like the others, registering a `TerminalProvider`), and
  `bollard` and the feature `docker` leave `caelumora-open-terminal`, which keeps
  `open_terminal_shared`. No new trait: `Terminal`, `TerminalReader`, `Shell` and `TerminalProvider`
  stay as they are. The cost: an operator changes the storage's `type` and its `image`, and a volume
  written by Open Terminal's user may need another owner under another image.
- **Beside it.** No migration; two ways of doing one thing, with a maintained 0.x client for the
  one whose reasons this issue takes away.

### How the files go

One mechanism for everything, the scripts, run as the image's user, so that what is written belongs
to whoever runs the commands:

- `list`: the entries by shell glob (`"$1"/*`, `"$1"/.[!.]*`, `"$1"/..?*`), for each `stat -c`, the
  fields and the name written with `printf '%s\0'`. NUL-separated: no output to parse, any name
  possible.
- `stat`, `delete` (`rm`, or `rmdir`, which refuses what is not empty by itself - better than
  today), `mkdir -p`, `move_entry`.
- `read`: the size, then `cat`, on stdout, which outside a terminal is multiplexed and binary-safe.
- `write`: `cat` from stdin into a temporary name, then `mv` - "at most `max_file_bytes`, or nothing
  is written", which Open Terminal does not manage, by Caelumora breaking off and removing it. Where
  the temporary name lies has to be settled: it must be on the volume, and so can be seen.

A script says what went wrong by its exit code, each mapped to a `TerminalError`; what the image's
programs print is not passed on, as no message of Open Terminal's is today.

The archive API would give `read` a size and a stream without any tool in the image, but needs a tar
reader and writer, a second way to the same files, and care over the owner of what it unpacks. It
stays the way out if binary streams over exec's stdin prove unreliable (below).

### How the commands go

Every command gets a directory of its own under `/run/caelumora/<id>`, a tmpfs the container is
started with, so that it exists whatever the image is and ends with the container, as the commands
do. Which scope a command belongs to stays where it is, in the storage's `Commands`.

- `start`: a detached exec whose script writes its process group, runs the command with its output
  appended (`>>`) to `output`, standard output and error in the order they came, and leaves the exit
  code in `exit`, by `mv`, so that it is whole or absent.
- `max_run_secs`: a watcher beside it, `sleep` and `kill`, needing no `timeout`.
- `output`: `tail -c +N`, offsets in bytes; waiting is a loop of the script, so that an exec is run
  per call and not per look.
- `kill`: a second exec, `kill -s KILL -- -<group>`.
- **The output is bounded** by the watcher: past the bound it empties `output` (`: > output`; the
  appending writer goes on at the new end) and adds what it dropped to `dropped`, so that offsets
  stay absolute and the answer can say `truncated`. A full tmpfs alone would not do: the command
  would fail with `ENOSPC`, which it did not do before.

These files lie in reach of the commands, which run as the same user: a command can forge an exit
code or another's output. Today's trust is no better - the server is in reach as well - and a
workspace's commands are all of one boundary, but it should be said in the module docs.

### The container

The entrypoint is replaced by a loop that waits (`trap 'exit 0' TERM; while :; do sleep 3600 & wait
$!; done`), so that a container is ready at once and any image works - an image's own entrypoint
does not run, deliberately. `HostConfig.init` puts Docker's init at PID 1, which collects the
orphans of killed commands and needs nothing in the image. The hard-coded `chown user:user` of the
volume becomes the image's own user, read from its configuration, and is not needed for root.
`network: none` is allowed; no key and no address are kept.

### To check before relying on it

Not tried yet; what the design leans on:

- whether the process an exec starts leads a process group of its own, as `runc` seems to arrange -
  if not, `setsid` joins the required programs;
- whether `bollard` half-closes the stdin of an attached exec, without which `write` never sees its
  end;
- what an exec per operation costs: every file operation becomes a process started through the
  daemon, which is more than an HTTP call, and the panel lists often;
- that a command is limited to 128 KiB in one argument (`MAX_ARG_STRLEN` on Linux): a longer one
  has to go in by stdin.

### Tests

The scripts are ordinary shell and are tested in `cargo test` against a temporary directory, with
no container, under each shell found on the machine. The Docker side is tested by hand, or under an
environment flag where a daemon is there, as for `open_terminal_docker` today.

## Open questions

1. `sh` or `bash` (decision 2)?
2. Instead of `open_terminal_docker`, or beside it (decision 3)?

## Done when

- A storage starts a workspace's container from any image with a shell and the programs named, with
  no server, key or port in it, also with `network: none`.
- An image lacking them is refused at the first start, naming what is missing.
- `Terminal` and `Shell` keep their contract there, a command killed with all it started, its
  output bounded and read by offset, and its run bounded by `max_run_secs` from inside the
  container.
- `docs/WORKSPACE.md`, `docs/CONFIGURATION.md` and `docs/CRATES.md` describe it, and what
  `open_terminal_docker`'s operators have to change is said.

## History

2026-11-08: Opened from the question what Open Terminal offers over Docker directly. The maintainer
chose shell scripts over a program of our own, accepting a shell as what an image has to bring.
