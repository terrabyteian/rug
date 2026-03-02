terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/services-slow-plan.tfstate"
  }

  required_providers {
    external = {
      source  = "hashicorp/external"
      version = "~> 2.0"
    }
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# The external data source runs its program during `terraform plan`,
# making the plan phase visibly slow (~10s).
data "external" "slow_check" {
  program = ["bash", "-c", "sleep 10 && echo '{\"status\": \"ready\"}'"]
}

resource "null_resource" "deploy" {
  triggers = {
    status = data.external.slow_check.result["status"]
  }
}
