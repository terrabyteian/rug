terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/infra-vpc.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "vpc" {
  triggers = {
    name = var.vpc_name
  }

  provisioner "local-exec" {
    command = "echo 'Creating VPC: ${var.vpc_name}'"
  }
}
