{ pkgs ? import <nixpkgs> { } }:

let
  dlopenLibraries = with pkgs; [
    libxkbcommon

    # GPU backend
    vulkan-loader
    # libGL

    # Window system
    wayland
    # xorg.libX11
    # xorg.libXcursor
    # xorg.libXi
  ];
in pkgs.mkShell {
  buildInputs = with pkgs; [
    alsa-lib
    udev
    pipewire
    # libclang
  ];
  nativeBuildInputs = with pkgs; [
    pkg-config
    rustPlatform.bindgenHook
    # wgsl-analyzer
    # cargo
    # rustc
  ];

  env.RUSTFLAGS = "-C link-arg=-Wl,-rpath,${pkgs.lib.makeLibraryPath dlopenLibraries}";
}
