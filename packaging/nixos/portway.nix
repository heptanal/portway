{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.portway;
  toml = pkgs.formats.toml { };
  generatedSettings = {
    listen = cfg.listenAddress;
    port = cfg.port;
    auth_mode = cfg.authMode;
    token_file = "/var/lib/portway/token";
    pairing_socket = "/run/portway/pair.sock";
    pairing_allowed_uids = cfg.pairingAllowedUids;
    backend = "uinput";
    max_clients = cfg.maxClients;
    pointer_sensitivity = cfg.pointerSensitivity;
    log_level = cfg.logLevel;
    pairing_code_ttl_seconds = cfg.pairingCodeTtlSeconds;
    session_ttl_seconds = cfg.sessionTtlSeconds;
  }
  // lib.optionalAttrs (cfg.tlsCertificate != null) {
    tls_cert = cfg.tlsCertificate;
    tls_key = cfg.tlsPrivateKey;
  }
  // cfg.extraSettings;
  configFile = toml.generate "portway.toml" generatedSettings;
in
{
  options.services.portway = {
    enable = lib.mkEnableOption "Portway remote input controller";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "Portway package to run.";
    };

    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address on which Portway listens.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 2721;
      description = "TCP port on which Portway listens.";
    };

    authMode = lib.mkOption {
      type = lib.types.enum [
        "token"
        "disabled"
      ];
      default = "token";
      description = "Controller authentication mode.";
    };

    pairingAllowedUids = lib.mkOption {
      type = lib.types.listOf lib.types.ints.u32;
      default = [ ];
      example = [ 1000 ];
      description = "Local user IDs allowed to run portway pair without privilege elevation.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the Portway port on all firewall interfaces.";
    };

    firewallInterfaces = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "wlp2s0" ];
      description = "Interfaces on which to open the Portway port.";
    };

    tlsCertificate = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/var/lib/portway/tls/cert.pem";
      description = "Runtime path to a PEM certificate chain; do not place private material in the Nix store.";
    };

    tlsPrivateKey = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/var/lib/portway/tls/key.pem";
      description = "Runtime path to the PEM private key.";
    };

    maxClients = lib.mkOption {
      type = lib.types.ints.between 1 8;
      default = 1;
      description = "Maximum simultaneous controllers.";
    };

    pointerSensitivity = lib.mkOption {
      type = lib.types.float;
      default = 1.0;
      description = "Server-side pointer sensitivity.";
    };

    pairingCodeTtlSeconds = lib.mkOption {
      type = lib.types.ints.between 30 3600;
      default = 300;
      description = "Lifetime of a temporary pairing code.";
    };

    sessionTtlSeconds = lib.mkOption {
      type = lib.types.ints.between 300 604800;
      default = 43200;
      description = "Lifetime of an authenticated browser session.";
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "info";
      description = "Tracing filter used by Portway.";
    };

    extraSettings = lib.mkOption {
      type = lib.types.attrs;
      default = { };
      description = "Additional or overriding Portway TOML settings. Security-sensitive overrides are the operator's responsibility.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = (cfg.tlsCertificate == null) == (cfg.tlsPrivateKey == null);
        message = "services.portway.tlsCertificate and tlsPrivateKey must be configured together";
      }
    ];

    boot.kernelModules = [ "uinput" ];

    users.groups.portway = { };
    users.users.portway = {
      isSystemUser = true;
      group = "portway";
      home = "/var/lib/portway";
      createHome = true;
    };

    services.udev.extraRules = ''
      KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="portway", MODE="0660"
    '';

    environment.etc."portway/config.toml".source = configFile;
    environment.systemPackages = [ cfg.package ];

    networking.firewall.allowedTCPPorts = lib.optionals cfg.openFirewall [ cfg.port ];
    networking.firewall.interfaces = lib.genAttrs cfg.firewallInterfaces (_: {
      allowedTCPPorts = [ cfg.port ];
    });

    systemd.services.portway = {
      description = "Portway remote input controller";
      documentation = [ "https://github.com/heptanal/portway" ];
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [
        "network-online.target"
        "systemd-modules-load.service"
      ];
      serviceConfig = {
        Type = "simple";
        User = "portway";
        Group = "portway";
        ExecStart = "${lib.getExe cfg.package} --config /etc/portway/config.toml serve";
        Restart = "always";
        RestartSec = 2;
        StateDirectory = "portway";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "portway";
        RuntimeDirectoryMode = "0755";
        UMask = "0077";
        TimeoutStopSec = 10;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        DevicePolicy = "closed";
        DeviceAllow = [ "/dev/uinput rw" ];
      };
    };
  };
}
