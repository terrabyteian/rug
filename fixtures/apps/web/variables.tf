variable "service_name" {
  description = "Name of the web service"
  type        = string
  default     = "my-web"
}

variable "port" {
  description = "Port the web service listens on"
  type        = number
  default     = 3000
}
