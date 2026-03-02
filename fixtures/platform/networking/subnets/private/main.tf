terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../../../../.state/platform-networking-subnets-private.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "private_subnets" {
  triggers = {
    cidr_block = var.cidr_block
  }

  provisioner "local-exec" {
    command = "echo 'Private subnets: ${var.cidr_block}'"
  }
}
