# NixOS module: services.supernote.*
# Wires up the webapp (long-running), the templater (morning timer), the
# ingest agent (15-minute timer), and the rclone mount of the Google Drive
# Supernote tree that they all read/write.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.supernote;
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.supernote-system;
  renderer = self.packages.${pkgs.stdenv.hostPlatform.system}.supernote-renderer;

  stateDir = "/var/lib/supernote";
  gdriveDir = "${stateDir}/gdrive";
  fontDir = "${cfg.fontPackage}/share/fonts/truetype";

  commonEnv = {
    SUPERNOTE_WEBAPP_URL = "http://127.0.0.1:${toString cfg.port}";
    SUPERNOTE_MYSTYLE_DIR = "${gdriveDir}/${cfg.mystyleSubdir}";
    SUPERNOTE_FONT_DIR = fontDir;
    SUPERNOTE_FONT_NAME = cfg.fontName;
  };

  hardening = {
    NoNewPrivileges = true;
    PrivateTmp = true;
    ProtectSystem = "strict";
    ProtectHome = true;
    ProtectClock = true;
    ProtectControlGroups = true;
    ProtectKernelLogs = true;
    ProtectKernelModules = true;
    ProtectKernelTunables = true;
    ProtectHostname = true;
    LockPersonality = true;
    RestrictRealtime = true;
    RestrictSUIDSGID = true;
    RestrictAddressFamilies = [
      "AF_INET"
      "AF_INET6"
      "AF_UNIX"
    ];
    SystemCallArchitectures = "native";
    ReadWritePaths = [ stateDir ];
  };
in
{
  options.services.supernote = {
    enable = lib.mkEnableOption "Supernote meeting/action automation";

    port = lib.mkOption {
      type = lib.types.port;
      default = 8130;
      description = "Webapp listen port (bound on 0.0.0.0).";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the webapp port in the firewall.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.path;
      description = ''
        EnvironmentFile with secrets (mode 600, not in the store).
        Must define ANTHROPIC_API_KEY.
      '';
    };

    driveRemote = lib.mkOption {
      type = lib.types.str;
      default = "gdrive:Supernote";
      description = "rclone remote:path of the Drive folder the device syncs to.";
    };

    rcloneConfigFile = lib.mkOption {
      type = lib.types.path;
      description = "rclone.conf containing the Drive remote (created interactively once).";
    };

    noteSubdir = lib.mkOption {
      type = lib.types.str;
      default = "Note";
      description = "Subdirectory of the synced tree holding .note files.";
    };

    mystyleSubdir = lib.mkOption {
      type = lib.types.str;
      default = "MyStyle";
      description = "Subdirectory of the synced tree holding template PNGs.";
    };

    templaterOnCalendar = lib.mkOption {
      type = lib.types.str;
      default = "*-*-* 06:30:00";
      description = "systemd OnCalendar spec for the morning template run.";
    };

    agentInterval = lib.mkOption {
      type = lib.types.str;
      default = "15min";
      description = "How often the ingest agent scans for new pages.";
    };

    researchDailyCap = lib.mkOption {
      type = lib.types.int;
      default = 5;
      description = "Maximum deep-research runs per day (cost guard).";
    };

    transcribeModel = lib.mkOption {
      type = lib.types.str;
      default = "claude-opus-4-8";
      description = "Model used for handwriting transcription.";
    };

    researchModel = lib.mkOption {
      type = lib.types.str;
      default = "claude-opus-4-8";
      description = "Model used for the deep-research pipeline.";
    };

    fontPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.liberation_ttf;
      description = "Font package used for template PNGs and research PDFs.";
    };

    fontName = lib.mkOption {
      type = lib.types.str;
      default = "LiberationSans";
      description = "Font family name (expects <name>-Regular.ttf / -Bold.ttf).";
    };
  };

  config = lib.mkIf cfg.enable {
    # CLI access to supernote-seed (and the other binaries) on the host.
    environment.systemPackages = [ pkg ];

    users.users.supernote = {
      isSystemUser = true;
      group = "supernote";
      home = stateDir;
    };
    users.groups.supernote = { };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ cfg.port ];

    systemd.tmpfiles.rules = [
      "d ${stateDir} 0750 supernote supernote -"
      "d ${gdriveDir} 0750 supernote supernote -"
      "d ${stateDir}/pages 0750 supernote supernote -"
    ];

    # ---- rclone mount of the Drive-synced Supernote tree -----------------
    systemd.services.supernote-gdrive = {
      description = "rclone mount of the Supernote Google Drive tree";
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      # Put the setuid fusermount3 wrapper first on PATH. The nixpkgs rclone
      # wrapper *appends* its own (non-setuid) fuse3 to PATH, which otherwise
      # wins and fails to mount as a non-root user (EPERM).
      path = [ "/run/wrappers" ];
      serviceConfig = {
        Type = "notify";
        User = "supernote";
        Group = "supernote";
        ExecStart = lib.concatStringsSep " " [
          "${pkgs.rclone}/bin/rclone mount"
          cfg.driveRemote
          gdriveDir
          "--config ${cfg.rcloneConfigFile}"
          "--vfs-cache-mode writes"
          "--cache-dir ${stateDir}/rclone-cache"
          "--dir-cache-time 30s"
          "--poll-interval 30s"
        ];
        ExecStop = "${pkgs.fuse}/bin/fusermount -u ${gdriveDir}";
        Restart = "on-failure";
        RestartSec = 10;
        DeviceAllow = [ "/dev/fuse rw" ];
        # This unit is deliberately NOT sandboxed, for two FUSE reasons:
        #  * NoNewPrivileges would strip the setuid bit from fusermount3, so
        #    the mount syscall returns EPERM.
        #  * PrivateTmp / ProtectHome / ReadWritePaths each give the unit a
        #    private mount namespace, trapping the FUSE mount there so the
        #    webapp/agent/templater units never see it. Running in the host
        #    namespace lets the (shared) mount propagate to them.
        # The app units carry the hardening instead.
      };
    };

    # ---- webapp -----------------------------------------------------------
    systemd.services.supernote-webapp = {
      description = "Supernote meeting/action webapp";
      wants = [ "network-online.target" ];
      after = [
        "network-online.target"
        "supernote-gdrive.service"
      ];
      wantedBy = [ "multi-user.target" ];
      environment = commonEnv // {
        SUPERNOTE_DB = "${stateDir}/supernote.sqlite3";
        SUPERNOTE_PAGES_DIR = "${stateDir}/pages";
        SUPERNOTE_LISTEN = "0.0.0.0:${toString cfg.port}";
        SUPERNOTE_GDRIVE_DIR = gdriveDir;
        SUPERNOTE_TRANSCRIBE_MODEL = cfg.transcribeModel;
        SUPERNOTE_RESEARCH_MODEL = cfg.researchModel;
        SUPERNOTE_RESEARCH_DAILY_CAP = toString cfg.researchDailyCap;
      };
      serviceConfig = hardening // {
        Type = "simple";
        User = "supernote";
        Group = "supernote";
        EnvironmentFile = cfg.environmentFile;
        ExecStart = "${pkg}/bin/supernote-webapp";
        Restart = "on-failure";
        RestartSec = 5;
        StateDirectory = "supernote";
      };
    };

    # ---- templater (morning timer) -----------------------------------------
    systemd.services.supernote-templater = {
      description = "Supernote morning template generation";
      after = [
        "supernote-webapp.service"
        "supernote-gdrive.service"
      ];
      requires = [
        "supernote-webapp.service"
        "supernote-gdrive.service"
      ];
      environment = commonEnv;
      serviceConfig = hardening // {
        Type = "oneshot";
        User = "supernote";
        Group = "supernote";
        ExecStart = "${pkg}/bin/supernote-templater";
      };
    };
    systemd.timers.supernote-templater = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.templaterOnCalendar;
        Persistent = true;
      };
    };

    # ---- ingest agent (15-minute timer) -------------------------------------
    systemd.services.supernote-agent = {
      description = "Supernote ingest agent";
      after = [
        "supernote-webapp.service"
        "supernote-gdrive.service"
      ];
      requires = [
        "supernote-webapp.service"
        "supernote-gdrive.service"
      ];
      environment = commonEnv // {
        SUPERNOTE_NOTE_DIR = "${gdriveDir}/${cfg.noteSubdir}";
        SUPERNOTE_RENDERER = "${renderer}/bin/supernote-render";
      };
      serviceConfig = hardening // {
        Type = "oneshot";
        User = "supernote";
        Group = "supernote";
        ExecStart = "${pkg}/bin/supernote-agent";
      };
    };
    systemd.timers.supernote-agent = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "5min";
        OnUnitActiveSec = cfg.agentInterval;
      };
    };
  };
}
