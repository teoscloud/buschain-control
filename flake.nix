# BusChain Control — packages, apps, home module, and dev shell.
{
  description = "BusChain Control — tray-resident PipeWire mixer + control surface";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      lib = pkgs.lib;

      eguiLibs = with pkgs; [
        libxkbcommon
        libGL
        wayland
        vulkan-loader
        libx11
        libxcursor
        libxi
        libxrandr
      ];

      # Shared libs commercial VST3/CLAP modules commonly dlopen (ldd / LSP / Ardour notes).
      # Keep this list in sync for: package wrapProgram, plugin-ui/dsp helpers, and nix develop.
      vst3PluginRuntimeLibs = with pkgs; [
        # Fonts / 2D UI
        freetype
        fontconfig
        cairo
        pango
        harfbuzz
        libpng
        zlib
        expat
        brotli
        # GL / X11 / Wayland (editors + OpenGL UIs)
        libGL
        libglvnd
        libx11
        libxext
        libxcursor
        libxi
        libxrandr
        libxrender
        libxcb
        xcbutil
        libxkbcommon
        wayland
        vulkan-loader
        # Audio / media helpers some plugins pull at load
        alsa-lib
        libsndfile
        # Network / licensing (e.g. Bertom → libcurl)
        curl
        openssl
        # Toolkit fallbacks (prefer static plugins; still unblocks many binaries)
        gtk3
        gdk-pixbuf
        # Misc
        dbus
        libuuid
        icu
      ];

      rustPlatform = pkgs.rustPlatform;

      commonNative = with pkgs; [
        pkg-config
        rustPlatform.bindgenHook
      ];

      commonBuild = with pkgs; [
        pipewire
        pulseaudio
        libpulseaudio
        dbus
        openssl
      ] ++ eguiLibs ++ vst3PluginRuntimeLibs;

      gtkMixer = import ./packaging/nix/gtk-mixer.nix { inherit pkgs; };
      gtkScrollStrip = import ./packaging/nix/gtk-scroll-strip.nix { inherit pkgs; };

      buschainControlPkg = rustPlatform.buildRustPackage {
        pname = "buschain-control";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = commonNative ++ [ pkgs.makeWrapper pkgs.copyDesktopItems ];
        buildInputs = commonBuild;
        dontCargoInstall = true;
        buildPhase = ''
          runHook preBuild
          # Builds app + tools (incl. buschain-plugin-ui / buschain-plugin-dsp).
          cargo build --release -p buschain-control -p buschain-tools
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out/bin $out/share/applications $out/share/icons/hicolor/256x256/apps
          mkdir -p $out/share/buschain-control/waybar

          install -m755 target/release/buschain-control $out/bin/
          install -m755 target/release/buschain-daemon $out/bin/
          install -m755 target/release/buschain-ctl $out/bin/
          install -m755 packaging/waybar/buschain-waybar $out/bin/buschain-waybar
          # VST3/CLAP helpers (native editor + per-track sandbox DSP).
          if [ -f target/release/buschain-plugin-ui ]; then
            install -m755 target/release/buschain-plugin-ui $out/bin/
          fi
          if [ -f target/release/buschain-plugin-dsp ]; then
            install -m755 target/release/buschain-plugin-dsp $out/bin/
          fi
          if [ -f target/release/buschain-plugin-surface ]; then
            install -m755 target/release/buschain-plugin-surface $out/bin/
          fi

          ln -s ${gtkMixer}/bin/buschain-mixer-gtk $out/bin/buschain-mixer-gtk
          ln -s ${gtkMixer}/bin/buschain-mixer $out/bin/buschain-mixer
          ln -s ${gtkScrollStrip}/bin/buschain-scroll-strip $out/bin/buschain-scroll-strip

          # Reference snippets for user rice (not auto-installed into ~/.config).
          install -m644 packaging/waybar/module.jsonc \
            $out/share/buschain-control/waybar/module.jsonc
          install -m644 packaging/waybar/style.css \
            $out/share/buschain-control/waybar/style.css

          mkdir -p $out/share/buschain-control/wireplumber/wireplumber.conf.d
          install -m644 pipewire/wireplumber/wireplumber.conf.d/51-buschain-seal-helpers.conf \
            $out/share/buschain-control/wireplumber/wireplumber.conf.d/
          install -m755 scripts/install-wireplumber-rules.sh \
            $out/share/buschain-control/install-wireplumber-rules.sh

          install -m644 packaging/wayland/buschain-control.desktop $out/share/applications/
          if [ -f assets/icons/buschain-control.png ]; then
            cp assets/icons/buschain-control.png $out/share/icons/hicolor/256x256/apps/buschain-control.png
          fi

          pluginLd=${lib.makeLibraryPath (eguiLibs ++ vst3PluginRuntimeLibs)}
          wrapProgram $out/bin/buschain-control \
            --prefix LD_LIBRARY_PATH : $pluginLd \
            --prefix PATH : $out/bin \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1
          wrapProgram $out/bin/buschain-daemon \
            --prefix PATH : ${lib.makeBinPath [ pkgs.pipewire pkgs.pulseaudio ]} \
            --prefix LD_LIBRARY_PATH : $pluginLd
          wrapProgram $out/bin/buschain-ctl \
            --prefix PATH : $out/bin \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1
          wrapProgram $out/bin/buschain-waybar \
            --prefix PATH : $out/bin:${lib.makeBinPath [ pkgs.pulseaudio pkgs.procps ]} \
            --set-default BUSCHAIN_CONTROL_CTL $out/bin/buschain-ctl \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1
          if [ -x $out/bin/buschain-plugin-ui ]; then
            wrapProgram $out/bin/buschain-plugin-ui \
              --prefix LD_LIBRARY_PATH : $pluginLd
          fi
          if [ -x $out/bin/buschain-plugin-dsp ]; then
            wrapProgram $out/bin/buschain-plugin-dsp \
              --prefix LD_LIBRARY_PATH : $pluginLd
          fi
          if [ -x $out/bin/buschain-plugin-surface ]; then
            wrapProgram $out/bin/buschain-plugin-surface \
              --prefix LD_LIBRARY_PATH : $pluginLd
          fi

          runHook postInstall
        '';
        doCheck = false;
        meta = with lib; {
          description = "BusChain Control tray mixer and PipeWire control surface";
          license = licenses.mit;
          platforms = platforms.linux;
          mainProgram = "buschain-control";
        };
      };
    in {
      packages.${system} = {
        default = buschainControlPkg;
        buschain-control = buschainControlPkg;
        buschain-daemon = buschainControlPkg;
        buschain-ctl = buschainControlPkg;
        buschain-waybar = buschainControlPkg;
        buschain-mixer-gtk = gtkMixer;
        buschain-scroll-strip = gtkScrollStrip;
      };

      apps.${system} = {
        default = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-control";
        };
        daemon = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-daemon";
        };
        ctl = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-ctl";
        };
      };

      # Optional: put bins on PATH. Waybar module/CSS stay in the user's rice.
      homeModules.buschain-control = { config, lib, pkgs, ... }:
        let cfg = config.services.buschain-control; in {
          options.services.buschain-control = {
            enable = lib.mkEnableOption "BusChain Control package on home.packages";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.buschain-control;
            };
          };
          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
            home.sessionVariables.BUSCHAIN_CONTROL_USE_GTK_MIXER = "1";
          };
        };

      nixosModules.buschain-control = self.homeModules.buschain-control;

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustPlatform.bindgenHook
          pkg-config
          gcc
          gnumake
          lv2
          lilv
          serd
          sord
          sratom
          zix
          libxcb
          pipewire
          pulseaudio
          libpulseaudio
          dbus
          gtk3
          gtk-layer-shell
          gobject-introspection
          (python3.withPackages (ps: [ ps.pygobject3 ]))
          # Wrapped mixer + scroll strip (gi + layer-shell) for waybar/tray.
          gtkMixer
          gtkScrollStrip
        ] ++ eguiLibs ++ vst3PluginRuntimeLibs;
        # Host + in-process VST3/CLAP + buschain-plugin-ui/dsp share this path.
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (eguiLibs ++ vst3PluginRuntimeLibs);
        shellHook = ''
          export CARGO_TARGET_DIR="$PWD/target"
          export LADSPA_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-reverb/build:$PWD/plugins/buschain-builtins/build:''${LADSPA_PATH:-}"
          export LV2_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-reverb/build:$PWD/plugins/buschain-builtins/build:''${LV2_PATH:-}"
          # Prefer freshly built helpers next to cargo target (plugin UI / sandbox DSP).
          export PATH="$PWD/target/debug:$PWD/target/release:$PWD/packaging/waybar:$PWD/packaging/scroll-strip:$PATH"
          export BUSCHAIN_CONTROL_CTL="$PWD/target/debug/buschain-ctl"
          # Pin the pygobject interpreter so tray/waybar children don't pick /usr/bin/python3.
          export BUSCHAIN_CONTROL_PYTHON="$(command -v python3)"
          # Checkout launcher runs legacy/*.py with that Python (live UI edits).
          export BUSCHAIN_CONTROL_MIXER="$PWD/packaging/mixer/buschain-mixer-gtk"
          export BUSCHAIN_CONTROL_MIXER_CSS="$PWD/packaging/mixer/legacy/style.css"
          export BUSCHAIN_CONTROL_SCROLL_STRIP_BIN="$PWD/packaging/scroll-strip/buschain-scroll-strip"
          export BUSCHAIN_CONTROL_USE_GTK_MIXER=1
          chmod +x "$PWD/packaging/mixer/buschain-mixer-gtk" "$PWD/packaging/waybar/buschain-waybar" "$PWD/packaging/scroll-strip/buschain-scroll-strip" 2>/dev/null || true
          echo "BusChain Control — cargo run  (plugins + ctl + tray + IPC)"
          echo "  hidden: cargo run -- --hidden"
          echo "  ctl:    cargo ctl -- status"
          echo "  popup:  QS → GTK → egui (USE_GTK_MIXER=0 skips GTK)"
          echo "  mixer:  $BUSCHAIN_CONTROL_MIXER"
          echo "  scroll: $BUSCHAIN_CONTROL_SCROLL_STRIP_BIN (opt-in SCROLL_STRIP=1)"
          echo "  python: $BUSCHAIN_CONTROL_PYTHON"
          echo "  vst3:   LD_LIBRARY_PATH includes freetype/cairo/curl/alsa/… for plugin dlopen"
          echo "  helpers: cargo build -p buschain-tools --bin buschain-plugin-ui --bin buschain-plugin-dsp --bin buschain-plugin-surface"
          echo "  window:  Wayland host (default); VST3 editors float on XWayland"
        '';
      };
    };
}
