terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/platform-networking.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Child of platform/ — also an intermediate node (subnets/ lives beneath it).
resource "null_resource" "networking" {
  triggers = {
    cidr = var.cidr_block
  }

  provisioner "local-exec" {
    command = "echo 'Configuring networking: ${var.cidr_block}'"
  }
}
