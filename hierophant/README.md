# "Hierophant" and "contemplant"

"Hierophant" and "contemplant" are our versions of "coordinator" and "worker", or
"master" and "slave".  The Hierophant delegates proofs to be computed by the
contemplants.

> "A hierophant is an interpreter of sacred mysteries and arcane principles."
[wikipedia](https://en.wikipedia.org/wiki/Hierophant)

> "Contemplant: One who contemplates."
[wikipedia](https://en.wiktionary.org/wiki/contemplant)

## Developing

When making a breaking change in Hierophant/Contemplant compatability, increment
the `CONTEMPLANT_VERSION` var in `network-lib/src/lib.rs`.  On each contemplant
connection, the Hierophant asserts that the `CONTEMPLANT_VERSION` that they have
is the same as the `CONTEMPLANT_VERSION` being passed in by the contemplant.

## Building

Install `protoc`: [https://protobuf.dev/installation/](https://protobuf.dev/installation/)

`cargo build`

## `old_testament.txt`

List of biblical names to randomly draw from if a contemplant is started without a name.
