terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../../.state/platform-networking-subnets.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Leaf beneath platform/networking/ — three levels deep.
resource "null_resource" "subnet" {
  count = var.subnet_count

  triggers = {
    index = count.index
  }

  provisioner "local-exec" {
    command = "echo 'Creating subnet ${count.index + 1} of ${var.subnet_count}'"
  }
}
