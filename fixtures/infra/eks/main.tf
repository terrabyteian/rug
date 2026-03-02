terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/infra-eks.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

resource "null_resource" "eks_cluster" {
  triggers = {
    cluster_name = var.cluster_name
  }

  provisioner "local-exec" {
    command = "echo 'Creating EKS cluster: ${var.cluster_name}'"
  }
}
