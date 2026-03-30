#!/bin/bash

crates=(
    wayle-cava
    wayle-hyprland
    wayle-media
    wayle-network
    wayle-notification
    wayle-power-profiles
    wayle-sysinfo
    wayle-systray
    wayle-wallpaper
    wayle-weather
)

cd "$(dirname "$0")"

for crate in "${crates[@]}"; do
    echo "$(date '+%H:%M:%S') Publishing $crate..."
    if cargo publish -p "$crate" --no-verify 2>&1; then
        echo "$(date '+%H:%M:%S') $crate published"
    else
        echo "$(date '+%H:%M:%S') $crate rate limited, waiting 10 minutes..."
        sleep 600
        echo "$(date '+%H:%M:%S') Retrying $crate..."
        cargo publish -p "$crate" --no-verify 2>&1
    fi
    sleep 15
done

echo "Done. All crates published."
