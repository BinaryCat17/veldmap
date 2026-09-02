# Operations in flight

How long work is killed, what a kill costs, and why the platform, not the
module, answers for a dead executor. The accounting itself — exchanges and
the terminal reply — is in [the bus page](bus-and-schema.md).

## Killing

The owner of an operation kills it with the generated stub
`crate::cancel::<service>::<topic>(&correlation_id)`. The stub exists
exactly for the calls whose topic is declared `cancellable: true`: killing
what is not declared cancellable is a compile error, not a silent refusal at
run time. The registry accounts for non-cancellable exchanges too, and knows
they may not be killed, so the ban does not rest on the missing stub alone.
This is an ABI call, not an event: a kill changes the host's state, like
freeing a resource, and answers at once. There is no `tasks` service.

A native service that panics takes the process with it in the release build
(`panic = "abort"`); in a debug build a panic takes only the task, which is
one more reason changes are checked with the release build.

A kill has no ceremony. A native executor is removed by aborting its future;
a wasm executor is trapped by the epoch interrupt, in the middle of any line,
after which its store is poisoned and the instance is raised anew, losing all
its state. Nothing is finished or unwound: this models a power cut.

The whole instance goes, and that is cheap precisely because events are
delivered to it one at a time (`Dispatcher::spawn_actor`): there is exactly
one event in flight — the one being killed — and no neighbouring work inside.
The instance is rebuilt, not the binary: the `Module` is compiled once and
reused, so the price is a new store plus `init`. What is lost is the
instance's `State`, and from that follows who may declare itself cancellable.

A module that outgrows itself ends the same way: an instance's linear memory
is capped by `INSTANCE_MEMORY_LIMIT`, `memory.grow` past it is refused, the
allocator inside the module traps, and it takes only itself, not the host.
That is a limit, not a working mode: the decoders of `image-tiler` count
their price in advance and refuse with a message long before it. The same
number caps a byte resource a module asks the host to keep on its behalf
(`alloc_cpu`): it may not delegate more than it may hold.

## What the platform does for a dead executor

- **The host runs the destructors.** A killed instance had none, so
  everything it owned is returned by the registry through the lease
  (`free_owned_by`) — as a system takes the descriptors from a process whose
  power was cut.
- **The terminal reply always comes.** The dead executor will not send it, so
  the host does: it publishes the `terminal:` topic with an empty payload,
  which decodes to default values. For the requester nothing changes: exactly
  one end to an operation, by the same topic, however it ended. There is no
  second channel "the task was cancelled", and the requester cannot tell a
  settled reply from a real one — it has nothing to treat them differently
  with.

An actor whose instance does not come back up (the rebuild itself failed)
does not call the dead store: it tries again on the next event, and if that
fails too, it settles the exchanges — the one it could not deliver included —
and returns the instance's resources. Requests still queued survive the fall
and are answered by the raised instance.

## Who may be cancellable

Correctness after a kill is a property of how the participants are built, not
of how carefully they finish. Cancellable is whoever has nothing to lose:
`image-tiler` keeps a memo of the parsed source between calls (the heavy slot
holds a NetCDF plane, the light one headers), but nothing that an answer
depends on, and its durable state — the tile cache — is owned by the
unkillable `tile-cache`. Everything that must survive a break is already on
disk by then: a `.part` file stays as the point of resumption, sidecars and
tiles are written atomically (a temporary file, then a rename), so a break
in the middle of a write leaves no half-file.

The tests of the registry (`veldcore/platform/host/core/src/tasks.rs`) hold
the sentence — the kill lands on the delivery it was issued for and on no
other — and the settling of started exchanges for a service that is gone; the
dispatcher's tests hold the kill rights: only the owner, only a cancellable
exchange, and the host's reply for the killed one.
