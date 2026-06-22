resource "aws_cloudwatch_log_group" "control_plane" {
  name              = "/flowplane/${var.environment}/control-plane"
  retention_in_days = var.log_retention_days
}

resource "aws_ecs_cluster" "this" {
  name = local.name

  setting {
    name  = "containerInsights"
    value = "enabled"
  }
}

data "aws_iam_policy_document" "ecs_task_assume" {
  statement {
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "task_execution" {
  name               = "${local.name}-task-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_task_assume.json
}

resource "aws_iam_role_policy_attachment" "task_execution" {
  role       = aws_iam_role.task_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

data "aws_iam_policy_document" "task_execution_secrets" {
  statement {
    actions = [
      "secretsmanager:GetSecretValue",
    ]

    resources = compact([
      var.secret_encryption_key_secret_arn,
      var.api_tls_cert_secret_arn,
      var.api_tls_key_secret_arn,
      var.xds_tls_cert_secret_arn,
      var.xds_tls_key_secret_arn,
      var.xds_tls_client_ca_secret_arn,
      var.cert_issuer_ca_cert_secret_arn,
      var.cert_issuer_ca_key_secret_arn,
      aws_secretsmanager_secret.db_password.arn,
      var.bootstrap_token_secret_arn,
    ])
  }

  dynamic "statement" {
    for_each = length(var.secret_kms_key_arns) == 0 ? [] : [1]

    content {
      actions   = ["kms:Decrypt"]
      resources = var.secret_kms_key_arns
    }
  }
}

resource "aws_iam_role_policy" "task_execution_secrets" {
  name   = "${local.name}-task-execution-secrets"
  role   = aws_iam_role.task_execution.id
  policy = data.aws_iam_policy_document.task_execution_secrets.json
}

resource "aws_iam_role" "task" {
  name               = "${local.name}-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_task_assume.json
}

resource "aws_ecs_task_definition" "control_plane" {
  family                   = "${local.name}-control-plane"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.cpu
  memory                   = var.memory
  execution_role_arn       = aws_iam_role.task_execution.arn
  task_role_arn            = aws_iam_role.task.arn

  # ponytail: ARM64/Graviton — the release image is built arm64 natively (x86 cross-build segfaults
  # under QEMU on Apple Silicon). Switch to X86_64 if you build the image on an x86 host instead.
  runtime_platform {
    cpu_architecture        = "ARM64"
    operating_system_family = "LINUX"
  }

  container_definitions = jsonencode([
    {
      name      = "control-plane"
      image     = var.control_plane_image
      essential = true

      entryPoint = ["/bin/sh", "-ec"]
      command    = [local.container_command]

      portMappings = [
        {
          name          = "api"
          containerPort = 8080
          hostPort      = 8080
          protocol      = "tcp"
        },
        {
          name          = "xds"
          containerPort = 18000
          hostPort      = 18000
          protocol      = "tcp"
        },
      ]

      environment = concat(
        [
          { name = "FLOWPLANE_API_ADDR", value = "0.0.0.0:8080" },
          { name = "FLOWPLANE_XDS_ADDR", value = "0.0.0.0:18000" },
          { name = "FLOWPLANE_LOG_FORMAT", value = "json" },
          { name = "FLOWPLANE_LOG", value = "info" },
          { name = "FLOWPLANE_OIDC_ISSUER", value = var.oidc_issuer },
          { name = "FLOWPLANE_OIDC_AUDIENCE", value = var.oidc_audience },
          { name = "FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN", value = var.cert_issuer_trust_domain },
          { name = "FLOWPLANE_DB_HOST", value = aws_db_instance.this.address },
        ],
        var.oidc_jwks_uri == "" ? [] : [{ name = "FLOWPLANE_OIDC_JWKS_URI", value = var.oidc_jwks_uri }],
      )

      secrets = concat(
        [
          { name = "FLOWPLANE_SECRET_ENCRYPTION_KEY", valueFrom = var.secret_encryption_key_secret_arn },
          { name = "FLOWPLANE_API_TLS_CERT_PEM", valueFrom = var.api_tls_cert_secret_arn },
          { name = "FLOWPLANE_API_TLS_KEY_PEM", valueFrom = var.api_tls_key_secret_arn },
          { name = "FLOWPLANE_XDS_TLS_CERT_PEM", valueFrom = var.xds_tls_cert_secret_arn },
          { name = "FLOWPLANE_XDS_TLS_KEY_PEM", valueFrom = var.xds_tls_key_secret_arn },
          { name = "FLOWPLANE_XDS_TLS_CLIENT_CA_PEM", valueFrom = var.xds_tls_client_ca_secret_arn },
          { name = "FLOWPLANE_CERT_ISSUER_CA_CERT_PEM", valueFrom = var.cert_issuer_ca_cert_secret_arn },
          { name = "FLOWPLANE_CERT_ISSUER_CA_KEY_PEM", valueFrom = var.cert_issuer_ca_key_secret_arn },
          { name = "FLOWPLANE_DB_PASSWORD", valueFrom = aws_secretsmanager_secret.db_password.arn },
        ],
        var.bootstrap_token_secret_arn == "" ? [] : [{ name = "FLOWPLANE_BOOTSTRAP_TOKEN", valueFrom = var.bootstrap_token_secret_arn }],
      )

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          awslogs-group         = aws_cloudwatch_log_group.control_plane.name
          awslogs-region        = var.aws_region
          awslogs-stream-prefix = "cp"
        }
      }
    }
  ])
}

resource "aws_ecs_service" "control_plane" {
  name            = "${local.name}-control-plane"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.control_plane.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = values(aws_subnet.private)[*].id
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "control-plane"
    container_port   = 8080
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.xds.arn
    container_name   = "control-plane"
    container_port   = 18000
  }

  depends_on = [
    aws_lb_listener.api_https,
    aws_lb_listener.xds_tcp,
    aws_vpc_endpoint.ecr_api,
    aws_vpc_endpoint.ecr_dkr,
    aws_vpc_endpoint.logs,
    aws_vpc_endpoint.secretsmanager,
    aws_vpc_endpoint.s3,
  ]
}
