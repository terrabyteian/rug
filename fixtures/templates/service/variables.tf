variable "service_name" {
  description = "Name of the service (fill in before use)"
  type        = string
  default     = "my-service"
}

variable "environment" {
  description = "Deployment environment"
  type        = string
  default     = "dev"
}
