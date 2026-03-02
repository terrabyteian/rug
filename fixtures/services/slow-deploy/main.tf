terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/services-slow-deploy.tfstate"
  }

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Simulates a slow deployment (e.g. waiting for an ECS service to stabilise).
resource "null_resource" "slow_deploy" {
  triggers = {
    deploy_id = var.deploy_id
  }

  provisioner "local-exec" {
    command = "echo 'Starting deployment ${var.deploy_id}...' && sleep 15 && echo 'Deployment complete.'"
  }
}
