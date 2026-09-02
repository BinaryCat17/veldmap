# The bus and the schema

The only way services talk to each other is by publishing an event,
fire-and-forget. There are no synchronous calls between modules; the only
synchronous calls go from a module into the host's state, through the ABI.

## The bus

- A topic is `<service>/<topic>`, the payload a protobuf message.
- Delivery goes to every subscriber of the topic. A topic marked `targeted:
  true` in the schema is delivered to one addressee, named by service name;
  an addressed event with no such subscriber is lost, like a publication into
  a topic nobody listens to.
- A reply to a request is another event; the requester matches it to its
  request by the correlation. It travels in the envelope
  (`EventEnvelope.correlation_id`), not as a field of the domain message:
  which topics are pairs is declared in the schema (`replies_to`), and the
  stubs of those topics take the correlation as a separate argument — the
  others have nowhere to put it. A module reads it with
  `veldsdk::correlation()`.
- Every subscriber is an actor with its own queue: its handlers run one at a
  time, in publication order. The queue is unbounded, so a publisher never
  waits — and a handler that cannot keep up falls behind silently, with the
  queue growing and the latency with it.
- The host stamps every delivered event with the publisher's name, and a
  module reads it with `veldsdk::event_publisher()`. Every service has a name,
  native modules alongside wasm ones; only the host itself has none.

The dispatcher (`veldcore/platform/host/core/src/dispatcher.rs`) keeps the
subscriptions, the names and the exchanges. It is testable without a runtime:
the queues are channels, and publishing puts an event into them synchronously.

## Exchanges and the terminal reply

A task is not a separate thing in the platform: **a task is an event in
flight**. Nothing opens or closes it explicitly — both facts are already in
the passing event, and the dispatcher keeps the accounting from them.

Accounting opens when **any** request with a declared reply is published, and
closes when its `terminal:` reply passes. The operation's id is the request's
correlation, its owner the publisher stamped by the host; the module reports
neither, so neither can drift from reality. `cancellable: true` is a property
of the record, not the reason to make one: what rests on the record is the
promise **the terminal reply always comes**. If the executor dies mid-work,
the host settles the exchange with the same topic from the same row of the
table; a publication into a topic with no subscriber is closed the same way,
by the host, since there is no executor to answer.

`terminal: true` is written only where a request has several replies and
there is a choice: `network/on_fs_download` has two, and the end of the work
is `on_fs_download_result`, not the progress; a schema with an ambiguous end
does not build. Under one correlation there can be several exchanges: a
request that asks the next service with the same id it will answer its own
requester with (`data-provider/on_open` → `network/on_open`; `data-library`
runs a download through `on_signed` into `network/on_fs_download` with one id)
keeps both alive, and they are
told apart by their terminal topic — the reply of the inner one does not close
the outer. There is no separate table of "is this terminal": every
publication tries to close an exchange, and a non-terminal one simply matches
nothing.

The knowledge is one-sided. "Intermediate" is a reliable prohibition — the
executor will send more, and `Correlator::take` and `Latest::settle` warn when
called on such a reply, because settling on progress loses the next reply and
the resource in it. "Terminal" is not permission to settle: the requester may
continue the same correlation with a further exchange, and only it knows
where its chain ends.

What the promise covers: an executor that is gone. A crashed or killed
instance loses its state whole, and every exchange it had started is settled
with its terminal — not only the one it fell on; an asynchronous module would
otherwise stay in debt, having accepted a request in one handler and answering
in another. Not covered, and both above the platform: a module that is alive
but does not answer (lost the correlation, returned early, waits for a reply
that will not come), and a native service that panics with `panic = "abort"`
and takes the process with it. A settled reply is empty, and an empty proto3
message is every field at its default — "all is well, and there is nothing";
the receiver should read an empty terminal reply as "no end was seen", not as
"it worked".

The FLOW table the host searches is generated from the schemas and sorted by
request, because the lookup is a binary search; `buildgen/tests/test_project.py`
holds both the sortedness and the completeness of that table, and the
dispatcher's own tests hold the settling for a missing subscriber.

## The schema as the source of truth

Topics are declared only in `schema.yaml`. From it the generator
(`buildgen/generate.py`) builds the module's `generated/` crate: the
`handle_event` dispatch from topic to handler with the payload type from the
schema; the subscription list for `get_subscriptions`; typed stubs
`crate::emit::*` for the module's own outputs and `crate::calls::<service>::*`
for declared dependencies; the list of intermediate replies among its
subscriptions, by which the SDK catches settling on progress. There are no
string topics in module code.

Before generation the schema is validated: every type exists; a topic is
declared by its producing service; a foreign topic has a payload type visible
to the consumer (the producer's package is declared in `dependencies`); the
input named in `replies_to` exists; an input with `cancellable: true` has a
reply; a request with several replies has an unambiguous terminal; `name:`
matches the directory; and the snapshot rules below. A topic's type is
declared once, by the producer; the consumer names the topic only, and the
type is derived from the producer's schema. `module/` in a type refers to the
module's own `types.proto` and only so — the same package written out in full
would look like a foreign one, and a foreign one needs a declared dependency.
This works for the topic's message itself; a nested field needs an `import`
in the `.proto`, which the wrap crate resolves only from its own directory and
`veldcore/interface/`.

```yaml
name: example
interface:
  inputs:
    on_do:
      type: module/DoRequest
      cancellable: true         # long work: it may be killed
  outputs:
    on_state:
      type: module/State
      snapshot: true            # the whole state: an unchanged one is not sent
    on_do_progress:
      type: module/Progress
      replies_to: on_do
    on_do_result:
      type: module/DoResult
      replies_to: on_do
      terminal: true            # the end of the operation, when replies are several
dependencies:
  fs:
    subs: [on_read_result]      # foreign outputs we subscribe to
    calls: [on_read]            # foreign inputs we publish into
hooks: [hook_event]
```

The handwritten part of a module is `src/module.rs`: `Config`, `State`,
`hook_init`, and free handler functions whose names are the keys of the
topics in the schema; `hook_event` runs after every handled event.

**A snapshot is not a command, and the schema tells them apart.** A topic
marked `snapshot: true` carries the whole state: what was not sent, the
subscriber no longer has. Such a topic is resent on every change of what it
describes, from many places, so the check "did it change" lives in the
generated stub, not at the call sites: the stub remembers the fingerprint of
the last body sent and does not publish a repeat (`veldsdk::snapshot`).
`crate::emit::resend::<topic>()` forgets the fingerprint, and the next send
goes out for sure — that is how `data-library` answers `on_list`. The sender
remembers, hence the one rule: a subscriber of a snapshot may not lose its
state. A killed and raised instance would lose it silently, and the sender
would not resend; so the schema does not combine `snapshot: true` with a
cancellable consumer, nor with `targeted` (one fingerprint, many addressees),
nor with a `replies_to` pair (a reply belongs to its request and must go out
even when identical).
