# This flake file is community maintained
{
  description = "nori: a fork of the niri scrollable-tiling Wayland compositor.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      revision = self.shortRev or self.dirtyShortRev or "unknown";
      nori-package =
        {
          lib,
          cairo,
          dbus,
          libGL,
          libdisplay-info_0_3,
          libinput,
          seatd,
          libxkbcommon,
          libgbm,
          pango,
          pipewire,
          pkg-config,
          rustPlatform,
          systemd,
          wayland,
          installShellFiles,
          withDbus ? true,
          withSystemd ? true,
          withScreencastSupport ? true,
          withDinit ? false,
        }:

        rustPlatform.buildRustPackage {
          pname = "nori";
          version = revision;

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./niri-config
              ./niri-ipc
              ./niri-visual-tests
              ./resources
              ./src
              ./Cargo.toml
              ./Cargo.lock
              ./build.rs
            ];
          };

          postPatch = ''
            patchShebangs resources/nori-session
            substituteInPlace resources/nori.service \
              --replace-fail 'ExecStart=nori' "ExecStart=$out/bin/nori"
          '';

          cargoLock = {
            # NOTE: This is only used for Git dependencies
            allowBuiltinFetchGit = true;
            lockFile = ./Cargo.lock;
          };

          strictDeps = true;

          nativeBuildInputs = [
            rustPlatform.bindgenHook
            pkg-config
            installShellFiles
          ];

          buildInputs =
            [
              cairo
              dbus
              libGL
              libdisplay-info_0_3
              libinput
              seatd
              libxkbcommon
              libgbm
              pango
              wayland
            ]
            ++ lib.optional (withDbus || withScreencastSupport || withSystemd) dbus
            ++ lib.optional withScreencastSupport pipewire
            # Also includes libudev
            ++ lib.optional withSystemd systemd;

          buildFeatures =
            lib.optional withDbus "dbus"
            ++ lib.optional withDinit "dinit"
            ++ lib.optional withScreencastSupport "xdp-gnome-screencast"
            ++ lib.optional withSystemd "systemd";
          buildNoDefaultFeatures = true;

          # ever since this commit:
          # https://github.com/niri-wm/niri/commit/771ea1e81557ffe7af9cbdbec161601575b64d81
          # niri now runs an actual instance of the real compositor (with a mock backend) during tests
          # and thus creates a real socket file in the runtime dir.
          # this is fine for our build, we just need to make sure it has a directory to write to.
          preCheck = ''
            export XDG_RUNTIME_DIR="$(mktemp -d)"
          '';

          checkFlags = [
            # These tests require the ability to access a "valid EGL Display", but that won't work
            # inside the Nix sandbox
            "--skip=::egl"
          ];

          postInstall =
            ''
              installShellCompletion --cmd nori \
                --bash <($out/bin/nori completions bash) \
                --fish <($out/bin/nori completions fish) \
                --nushell <($out/bin/nori completions nushell) \
                --zsh <($out/bin/nori completions zsh)

              install -Dm644 resources/nori.desktop -t $out/share/wayland-sessions
              install -Dm644 resources/nori-portals.conf -t $out/share/xdg-desktop-portal
            ''
            + lib.optionalString withSystemd ''
              install -Dm755 resources/nori-session $out/bin/nori-session
              install -Dm644 resources/nori{.service,-shutdown.target} -t $out/lib/systemd/user
            '';

          env = {
            # Force linking with libEGL and libwayland-client so they end up in RPATH and
            # can be discovered by `dlopen()`
            RUSTFLAGS = toString (
              map (arg: "-C link-arg=" + arg) [
                "-Wl,--push-state,--no-as-needed"
                "-lEGL"
                "-lwayland-client"
                "-Wl,--pop-state"
              ]
            );
            NORI_BUILD_COMMIT = revision;
          };

          passthru = {
            providedSessions = [ "nori" ];
          };

          meta = {
            description = "Scrollable-tiling Wayland compositor";
            homepage = "https://github.com/Deliveos/nori";
            license = lib.licenses.gpl3Only;
            mainProgram = "nori";
            platforms = lib.platforms.linux;
          };
        };

      inherit (nixpkgs) lib;
      # Support all Linux systems that the nixpkgs flake exposes
      systems = lib.intersectLists lib.systems.flakeExposed lib.platforms.linux;

      forAllSystems = lib.genAttrs systems;
      nixpkgsFor = forAllSystems (system: nixpkgs.legacyPackages.${system});
    in
    {
      checks = forAllSystems (system: {
        # We use the debug build here to save a bit of time
        inherit (self.packages.${system}) nori-debug;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
          rustfmt' = pkgs.rustfmt.override { asNightly = true; };
          inherit (self.packages.${system}) nori;
        in
        {
          default = pkgs.mkShell {
            packages = builtins.attrValues {
              inherit (pkgs)
                rustc
                cargo
                clippy
                cargo-insta
                ;
              inherit rustfmt';
            };

            nativeBuildInputs = [
              pkgs.rustPlatform.bindgenHook
              pkgs.pkg-config
              pkgs.wrapGAppsHook4 # For `niri-visual-tests`
            ];

            buildInputs = nori.buildInputs ++ [
              pkgs.libadwaita # For `niri-visual-tests`
            ];

            env = {
              # WARN: Do not overwrite this variable in your shell!
              # It is required for `dlopen()` to work on some libraries; see the comment
              # in the package expression
              #
              # This should only be set with `RUSTFLAGS="$RUSTFLAGS -C your-flags"`
              RUSTFLAGS = nori.RUSTFLAGS;
            };
          };
        }
      );

      formatter = forAllSystems (system: nixpkgsFor.${system}.nixfmt-rfc-style);

      packages = forAllSystems (
        system:
        let
          nori = nixpkgsFor.${system}.callPackage nori-package { };
        in
        {
          inherit nori;

          # NOTE: This is for development purposes only
          #
          # It is primarily to help with quickly iterating on
          # changes made to the above expression - though it is
          # also not stripped in order to better debug niri itself
          nori-debug = nori.overrideAttrs (
            newAttrs: oldAttrs: {
              pname = oldAttrs.pname + "-debug";

              cargoBuildType = "debug";
              cargoCheckType = newAttrs.cargoBuildType;

              dontStrip = true;
            }
          );

          default = nori;
        }
      );

      overlays.default = final: _: {
        nori = final.callPackage nori-package { };
      };
    };
}
