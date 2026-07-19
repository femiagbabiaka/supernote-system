{ lib, python3Packages }:

let
  potracer = python3Packages.buildPythonPackage rec {
    pname = "potracer";
    version = "0.0.4";
    pyproject = true;
    src = python3Packages.fetchPypi {
      inherit pname version;
      sha256 = "32cbdb984446066bcfbe8b600142a54b90fa6da274b69219473205d6e4c09713";
    };
    build-system = [ python3Packages.setuptools ];
    dependencies = with python3Packages; [ numpy ];
    doCheck = false;
  };

  supernotelib = python3Packages.buildPythonPackage rec {
    pname = "supernotelib";
    version = "0.7.1";
    pyproject = true;
    src = python3Packages.fetchPypi {
      inherit pname version;
      sha256 = "566ff148104a7db97d8528eb84bd5cd8912fe8363e08e57597594089cf44703b";
    };
    build-system = [ python3Packages.hatchling ];
    dependencies = with python3Packages; [
      colour
      fusepy
      numpy
      pillow
      potracer
      pypng
      reportlab
      svglib
      svgwrite
    ];
    doCheck = false;
  };
in
python3Packages.buildPythonApplication {
  pname = "supernote-render";
  version = "0.1.0";
  format = "other";
  dontUnpack = true;

  dependencies = [
    supernotelib
    python3Packages.numpy
    python3Packages.pillow
  ];

  installPhase = ''
    install -Dm755 ${../python/render_note.py} $out/bin/supernote-render
  '';

  meta = {
    description = "Supernote .note → per-page ink-layer PNG renderer (wraps supernotelib)";
    license = lib.licenses.mit;
    mainProgram = "supernote-render";
  };
}
