{ pkgs }:

# Always-on GTK layer-shell scroll strip (Waybar Master HW hover notches).
let
  srcDir = ../scroll-strip/legacy;
  py = pkgs.python3.withPackages (ps: [ ps.pygobject3 ]);
in
pkgs.stdenv.mkDerivation {
  pname = "buschain-scroll-strip";
  version = "0.1.0";
  src = srcDir;

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
    mkdir -p $out/share/buschain-scroll-strip
    cp -f buschain-scroll-strip.py $out/share/buschain-scroll-strip/
    chmod +x $out/share/buschain-scroll-strip/buschain-scroll-strip.py
    runHook postInstall
  '';

  postFixup = ''
    mkdir -p $out/bin
    makeWrapper ${py}/bin/python3 $out/bin/buschain-scroll-strip \
      "''${gappsWrapperArgs[@]}" \
      --add-flags $out/share/buschain-scroll-strip/buschain-scroll-strip.py
  '';

  meta = {
    description = "BusChain Control Master HW scroll strip (layer-shell)";
    mainProgram = "buschain-scroll-strip";
  };
}
