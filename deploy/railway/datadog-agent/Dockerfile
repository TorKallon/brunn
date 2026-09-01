FROM gcr.io/datadoghq/agent:7.81.2@sha256:44bfe2c475d9467e78e73e4327148d7e3902da5a48cbb1b0f6b84b01283e89d5

# The vendor Agent's s6 supervisor requires root during container startup.
# trivy:ignore:AVD-DS-0002
USER root

COPY deploy/railway/datadog-agent/datadog.yaml /etc/datadog-agent/datadog.yaml
COPY deploy/railway/datadog-agent/http_check.yaml /etc/datadog-agent/conf.d/http_check.d/conf.yaml
COPY deploy/railway/datadog-agent/syslog.yaml /etc/datadog-agent/conf.d/syslog.d/conf.yaml

EXPOSE 8125/udp
EXPOSE 514/udp
EXPOSE 515/udp
