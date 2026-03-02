terraform {
  required_version = ">= 1.0"

  backend "local" {
    path = "../../.state/services-slow-plan-2.tfstate"
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

# Simulates fetching remote dependency state during plan:
# e.g. querying SSM parameters, checking ACM cert validity, probing service endpoints.
# Each data source runs concurrently — total plan time ≈ max(sleep) ≈ 40s.

data "external" "db_config" {
  program = ["bash", "-c", "sleep 40 && echo '{\"endpoint\": \"db.internal:5432\", \"replica\": \"db-ro.internal:5432\"}'"]
}

data "external" "cache_config" {
  program = ["bash", "-c", "sleep 25 && echo '{\"primary\": \"cache.internal:6379\", \"reader\": \"cache-ro.internal:6379\"}'"]
}

data "external" "cert_status" {
  program = ["bash", "-c", "sleep 35 && echo '{\"arn\": \"arn:aws:acm:us-east-1:123456789:certificate/fake-cert\", \"status\": \"ISSUED\"}'"]
}

resource "null_resource" "api" {
  triggers = {
    db_endpoint    = data.external.db_config.result["endpoint"]
    cache_primary  = data.external.cache_config.result["primary"]
  }
}

resource "null_resource" "worker" {
  triggers = {
    db_replica    = data.external.db_config.result["replica"]
    cache_reader  = data.external.cache_config.result["reader"]
  }
}

resource "null_resource" "ingress" {
  triggers = {
    cert_arn    = data.external.cert_status.result["arn"]
    cert_status = data.external.cert_status.result["status"]
  }
}

resource "null_resource" "service_mesh" {
  triggers = {
    api_id     = null_resource.api.id
    worker_id  = null_resource.worker.id
    ingress_id = null_resource.ingress.id
  }
}
