terraform {
  required_version = ">= 1.7"

  # S3 backend pointed at local MinIO.
  # Start MinIO first: docker compose up -d  (in fixtures/remote-state/)
  # Then: tofu init -reconfigure
  backend "s3" {
    bucket   = "rug-fixtures"
    key      = "remote-state/demo/terraform.tfstate"
    region   = "us-east-1"

    # MinIO local endpoint
    endpoint   = "http://localhost:9000"
    access_key = "minioadmin"
    secret_key = "minioadmin"

    # Required for non-AWS S3-compatible stores
    skip_credentials_validation = true
    skip_metadata_api_check     = true
    skip_region_validation      = true
    force_path_style            = true

    # S3-native locking (OpenTofu 1.7+): writes a .tflock object next to the
    # state file in MinIO instead of using DynamoDB. Cancel a running plan to
    # leave a stale lock behind and test force-unlock.
    use_lockfile = true
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

variable "environment" {
  default = "dev"
}

# Slow data source so a plan takes ~30s — long enough to cancel mid-run,
# which leaves the .tflock object in MinIO as a stale lock.
data "external" "config_check" {
  program = ["bash", "-c", "sleep 30 && echo '{\"endpoint\": \"db.internal:5432\", \"region\": \"us-east-1\"}'"]
}

resource "null_resource" "app" {
  triggers = {
    env      = var.environment
    endpoint = data.external.config_check.result["endpoint"]
  }

  provisioner "local-exec" {
    command = "echo 'Deploying app to ${var.environment}'"
  }
}

resource "null_resource" "db" {
  triggers = {
    env    = var.environment
    region = data.external.config_check.result["region"]
  }

  provisioner "local-exec" {
    command = "echo 'Provisioning database in ${var.environment}'"
  }
}

output "app_id" {
  value = null_resource.app.id
}
