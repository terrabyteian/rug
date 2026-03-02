variable "service_name" {
  description = "Name of the service being deployed"
  type        = string
  default     = "my-service"
}

variable "deploy_id" {
  description = "Unique deployment ID (change to force re-apply)"
  type        = string
  default     = "v1"
}
