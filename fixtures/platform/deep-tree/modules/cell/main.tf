# Library module — one instance per for_each key in the root "cell" module,
# including a dotted for_each key ("us.east-1") to exercise bracket-aware
# module-address parsing (module.cell["us.east-1"].null_resource.node).

resource "null_resource" "node" {}

resource "null_resource" "shard" {
  for_each = toset(["a", "b"])
}
