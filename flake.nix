{
  description = "Portway remote input controller";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain = pkgs.rust-bin.stable."1.97.0".minimal;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
          portway = pkgs.callPackage ./packaging/nixos/package.nix {
            inherit rustPlatform;
          };
        in
        {
          inherit portway;
          default = portway;
        }
      );

      nixosModules = {
        portway = { lib, pkgs, ... }: {
          imports = [ ./packaging/nixos/portway.nix ];
          services.portway.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.portway;
        };
        default = self.nixosModules.portway;
      };

      checks = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          evaluated = nixpkgs.lib.nixosSystem {
            inherit system;
            modules = [
              self.nixosModules.default
              {
                services.portway = {
                  enable = true;
                  listenAddress = "127.0.0.1";
                };
                system.stateVersion = "25.11";
              }
            ];
          };
          service = evaluated.config.systemd.services.portway;
        in
        {
          nixos-module =
            assert service.serviceConfig.User == "portway";
            assert service.serviceConfig.DeviceAllow == [ "/dev/uinput rw" ];
            pkgs.runCommand "portway-nixos-module-check" { } ''
              touch $out
            '';
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
