# Library module — no backend block, no state file.
# Should be excluded by default from rug's module list.

variable "vpc_cidr" {
  description = "CIDR block for the VPC"
  type        = string
}

variable "subnet_cidrs" {
  description = "List of subnet CIDR blocks"
  type        = list(string)
  default     = []
}

output "vpc_cidr" {
  value = var.vpc_cidr
}

output "subnet_count" {
  value = length(var.subnet_cidrs)
}
