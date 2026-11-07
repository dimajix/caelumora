# The example seed

Demo data (issue 105): component bundles, personalities, session templates and skills that a binary
puts into its stores at startup, when `seed` in its configuration names this directory (see
`examples/config.yaml`). Every `.yaml` file in it and its subdirectories is a seed file, read in the
order of the paths; this one is not. Each holds one document, in a directory named after the section
it is in, and is named after the document's name.

Every document is stored under the id it has here, and only if the store has no document with that
id yet - so loading it again changes nothing, and a document changed in the store stays as it is.
The documents are written as the store keeps them; fields left out take their defaults. Everything
here belongs to the system: every user may use it, only the system may change it.

- **Session templates** refer to personalities by id. Within a template, channels refer to the
  template's participants by name; "user" is whoever creates the session. A channel's approver is no
  participant but one of its internal actors (issue 420): a channel has those it names and no
  others, and one without an approver approves nothing - tools that ask for approval do not work in
  it.
- **Skills** (issue 22) lie in the global scope: every agent whose personality has the `skills`
  component finds them, loads them when a task calls for one, and a member gives one to an agent
  with `/skill @agent <name>`.
- **Component bundles** (issues 270, 455) are entities like the personalities, found by their name
  in their scope. A personality includes one with `kind: ref` and `params: { name: ... }`, and gets
  all of its components there, in their order: what every agent should have is written once, and an
  addition reaches every personality that includes it. A component of an included one must not be
  written again beside it.

Where a document refers to another - a personality to a bundle, a template to a personality -, it
is found when an agent or a session is made, not when the seed is read: the order of the files does
not matter.
