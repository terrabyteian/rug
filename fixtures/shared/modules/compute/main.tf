# Reusable compute module — no backend block, not a root module.
variable "instance_type" {}
variable "ami_id" {}

resource "null_resource" "compute" {
  triggers = {
    instance_type = var.instance_type
    ami_id        = var.ami_id
  }
}
