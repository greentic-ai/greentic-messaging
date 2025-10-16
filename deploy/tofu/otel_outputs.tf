output "otel_exporter_otlp_endpoint" {
  description = "Endpoint applications should use for OTLP gRPC/HTTP exports."
  value       = var.otel_exporter_otlp_endpoint
}

variable "otel_exporter_otlp_endpoint" {
  description = "Fully-qualified OTLP endpoint (ex: https://otel.example.com:4317)."
  type        = string
}
