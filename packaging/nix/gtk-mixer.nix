{ pkgs }:

# GTK layer-shell mixer panel (waybar / tray popup).
let
  mixerSrc = ../mixer/legacy;
  py = pkgs.python3.withPackages (ps: [ ps.pygobject3 ]);
in
pkgs.stdenv.mkDerivation {
  pname = "buschain-mixer-gtk";
  version = "0.1.0";
  src = mixerSrc;

  nativeBuildInputs = [
    pkgs.wrapGAppsHook3
    pkgs.makeWrapper
    pkgs.gobject-introspection
  ];

  buildInputs = [
    py
    pkgs.gtk3
    pkgs.gtk-layer-shell
    pkgs.cairo
    pkgs.pango
    pkgs.gdk-pixbuf
    pkgs.atk
    pkgs.harfbuzz
  ];

  dontWrapGApps = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/share/buschain-mixer
    cp -f buschain-mixer.py style.css $out/share/buschain-mixer/
    chmod +x $out/share/buschain-mixer/buschain-mixer.py
    runHook postInstall
  '';

  postFixup = ''
    mkdir -p $out/bin
    makeWrapper ${py}/bin/python3 $out/bin/buschain-mixer-gtk \
      "''${gappsWrapperArgs[@]}" \
      --set BUSCHAIN_CONTROL_MIXER_CSS $out/share/buschain-mixer/style.css \
      --run 'if [ -z "''${BUSCHAIN_CONTROL_CTL:-}" ]; then
        for _c in \
          "''${HOME}/Projects/buschain-control/target/debug/buschain-ctl" \
          "''${HOME}/Projects/buschain-control/target/release/buschain-ctl"
        do
          if [ -x "$_c" ]; then export BUSCHAIN_CONTROL_CTL="$_c"; break; fi
        done
      fi' \
      --add-flags $out/share/buschain-mixer/buschain-mixer.py
    ln -sf buschain-mixer-gtk $out/bin/buschain-mixer
  '';

  meta = {
    description = "BusChain Control GTK layer-shell mixer popup";
    mainProgram = "buschain-mixer-gtk";
  };
}
