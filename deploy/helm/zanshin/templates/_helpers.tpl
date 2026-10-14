{{- define "zanshin.name" -}}{{ default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}{{- end }}
{{- define "zanshin.fullname" -}}
{{- if .Values.fullnameOverride }}{{ .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}{{ printf "%s-%s" .Release.Name (include "zanshin.name" .) | trunc 63 | trimSuffix "-" }}{{- end }}
{{- end }}
{{- define "zanshin.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
app.kubernetes.io/name: {{ include "zanshin.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}
{{- define "zanshin.selectorLabels" -}}
app.kubernetes.io/name: {{ include "zanshin.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}
{{- define "zanshin.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}{{ default (include "zanshin.fullname" .) .Values.serviceAccount.name }}{{ else }}{{ default "default" .Values.serviceAccount.name }}{{ end }}
{{- end }}
{{- define "zanshin.secretName" -}}{{ default (include "zanshin.fullname" .) .Values.secrets.existingSecret }}{{- end }}
{{- define "zanshin.image" -}}{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}{{- end }}
{{/* env block shared by the Deployment and the migration Job */}}
{{- define "zanshin.env" -}}
- name: ZANSHIN__SERVER__HTTP_ADDR
  value: 0.0.0.0:{{ .Values.service.httpPort }}
- name: ZANSHIN__SERVER__GRPC_ADDR
  value: 0.0.0.0:{{ .Values.service.grpcPort }}
- name: ZANSHIN__SERVER__CORS_ORIGINS
  value: {{ printf "[%q]" .Values.publicUrl | quote }}
- name: ZANSHIN_FILE__DATABASE__URL
  value: /etc/zanshin/secrets/{{ .Values.secrets.keys.databaseUrl }}
- name: ZANSHIN_FILE__REDIS__URL
  value: /etc/zanshin/secrets/{{ .Values.secrets.keys.redisUrl }}
- name: ZANSHIN_FILE__CLICKHOUSE__PASSWORD
  value: /etc/zanshin/secrets/{{ .Values.secrets.keys.clickhousePassword }}
- name: ZANSHIN_FILE__AUTH__JWT_SECRET
  value: /etc/zanshin/secrets/{{ .Values.secrets.keys.jwtSecret }}
- name: ZANSHIN_FILE__ADMIN__BOOTSTRAP_KEY
  value: /etc/zanshin/secrets/{{ .Values.secrets.keys.adminBootstrapKey }}
- name: ZANSHIN__TELEMETRY__RESOURCE__K8S_NAMESPACE_NAME
  value: {{ .Release.Namespace | quote }}
- name: ZANSHIN__TELEMETRY__RESOURCE__K8S_POD_NAME
  valueFrom: { fieldRef: { fieldPath: metadata.name } }
{{- end }}
{{- define "zanshin.volumes" -}}
- name: secrets
  secret:
    secretName: {{ include "zanshin.secretName" . }}
    defaultMode: 0400
- name: tmp
  emptyDir: {}
{{- end }}
{{- define "zanshin.volumeMounts" -}}
- name: secrets
  mountPath: /etc/zanshin/secrets
  readOnly: true
- name: tmp
  mountPath: /tmp
{{- end }}
