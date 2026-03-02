# Reusable IAM roles module — no backend block, not a root module.
variable "role_name" {}

resource "null_resource" "iam_role" {
  triggers = {
    role_name = var.role_name
  }
}
