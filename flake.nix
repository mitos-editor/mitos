{
  description = "A post-modern text editor.";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
    ...
  }: let
    inherit (nixpkgs) lib;
    eachSystem = lib.genAttrs lib.systems.flakeExposed;
    pkgsFor = eachSystem (system:
      import nixpkgs {
        localSystem.system = system;
        overlays = [self.overlays.mitos];
      });
    gitRev = self.rev or self.dirtyRev or null;
  in {
    packages = eachSystem (system: {
      inherit (pkgsFor.${system}) mitos;
      /*
      The default Mitos build. Uses the latest stable Rust toolchain, and unstable
      nixpkgs.

      The build inputs can be overridden with the following:

      packages.${system}.default.override { rustPlatform = newPlatform; };

      Overriding a derivation attribute can be done as well:

      packages.${system}.default.overrideAttrs { buildType = "debug"; };
      */
      default = self.packages.${system}.mitos;
    });

    checks = self.packages;

    # Devshell behavior is preserved.
    devShells =
      lib.mapAttrs (system: pkgs: {
        default = let
          commonRustFlagsEnv = "-C link-arg=-fuse-ld=lld -C target-cpu=native --cfg tokio_unstable";
          platformRustFlagsEnv = lib.optionalString pkgs.stdenv.hostPlatform.isLinux "-Clink-arg=-Wl,--no-rosegment";
        in
          pkgs.mkShell {
            inputsFrom = [
              (self.checks.${system}.mitos.override {
                includeGrammarIf = _: false;
              })
            ];
            nativeBuildInputs = with pkgs;
              [
                lld
                cargo-flamegraph
                rust-analyzer
                rustfmt
                mdbook
              ]
              ++ (lib.optional (stdenv.hostPlatform.isx86_64 && stdenv.hostPlatform.isLinux) cargo-tarpaulin)
              ++ (lib.optional stdenv.hostPlatform.isLinux lldb);
            shellHook = ''
              export RUST_BACKTRACE="1"
              export RUSTFLAGS="''${RUSTFLAGS:-""} ${commonRustFlagsEnv} ${platformRustFlagsEnv}"
            '';
          };
      })
      pkgsFor;

    overlays = {
      mitos = final: prev: {
        mitos = final.callPackage ./default.nix {inherit gitRev;};
      };

      default = self.overlays.mitos;
    };
  };
}
