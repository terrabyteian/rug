variable "service_name" {
  description = "Name of the API service"
  type        = string
  default     = "my-api"
}

variable "image_tag" {
  description = "Docker image tag to deploy"
  type        = string
  default     = "latest"
}
