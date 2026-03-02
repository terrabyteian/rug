terraform {
  required_version = ">= 1.0"

  required_providers {
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }
}

# Root-level resources

resource "null_resource" "app_server" {}

resource "null_resource" "worker" {
  count = 3
}

resource "null_resource" "az_node" {
  for_each = toset(["us-east-1a", "us-east-1b", "us-west-2a"])
}

resource "null_resource" "db_primary" {}

resource "null_resource" "db_replica" {
  count = 2
}

resource "null_resource" "cache" {
  for_each = toset(["redis-0", "redis-1"])
}

data "null_data_source" "config" {}

# Child modules (definitions omitted — state is hand-crafted for fixture purposes)

module "networking" {
  source = "./modules/networking"
}

module "compute" {
  source = "./modules/compute"
}

module "dns" {
  source = "./modules/dns"
}

module "iam" {
  source = "./modules/iam"
}
