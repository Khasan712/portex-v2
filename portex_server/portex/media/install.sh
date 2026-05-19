#!/bin/bash

set -e

# GitHub Release URL
URL_PREFIX="https://github.com/Khasan712/portex-connecter/releases/download/0.0.1"
INSTALL_DIR="/usr/local/bin"

# Ensure INSTALL_DIR exists
if [ ! -d "$INSTALL_DIR" ]; then
  echo "$INSTALL_DIR does not exist, creating it."
  sudo mkdir -p "$INSTALL_DIR"
fi

# Detect platform and architecture
case "$(uname -sm)" in
  "Darwin x86_64") FILENAME="portex-darwin-amd64" ;;
  "Darwin arm64") FILENAME="portex-darwin-arm64" ;;
  *) echo "Unsupported architecture: $(uname -sm)" >&2; exit 1 ;;
esac

echo "Downloading $FILENAME from GitHub release"

# Get the file size (in bytes) using a HEAD request
FILE_SIZE=$(curl -sI "$URL_PREFIX/$FILENAME" | grep -i "Content-Length" | awk '{print $2}' | tr -d '\r')

# Check if file size is available
if [ -z "$FILE_SIZE" ]; then
  echo "Failed to retrieve file size" >&2
  exit 1
fi

# Convert file size to MB for display purposes
TOTAL_MB=$(echo "scale=2; $FILE_SIZE/1024/1024" | bc)

# Function to show progress bar with MB and percentage
download_with_progress() {
  local url="$1"
  local output="$2"

  # Initialize downloaded size variable
  DOWNLOADED=0

  # Use curl to download the file and show progress
  curl -L "$url" -o "$output" --progress-bar | while read -r line; do
    # Get the current downloaded size (in bytes)
    DOWNLOADED=$(curl -sI "$url" | grep -i "Content-Length" | awk '{print $2}' | tr -d '\r')

    # Calculate the percentage
    PERCENTAGE=$(( 100 * DOWNLOADED / FILE_SIZE ))

    # Convert downloaded size to MB
    DOWNLOAD_MB=$(echo "scale=2; $DOWNLOADED/1024/1024" | bc)

    # Create progress bar
    BAR_LENGTH=$((PERCENTAGE / 2))  # 100% is equal to 50 '#' characters
    BAR=$(printf "%-${BAR_LENGTH}s" "#" | tr " " "#")

    # Show progress bar with percentage and MB
    echo -ne "[$BAR] $PERCENTAGE% ($DOWNLOAD_MB MB / $TOTAL_MB MB)\r"
  done
}

# Start download with progress
if ! download_with_progress "$URL_PREFIX/$FILENAME" "$INSTALL_DIR/portex"; then
  echo -e "\nFailed to download from GitHub; check the URL and try again" >&2
  exit 1
fi

# Create a symlink for platform-agnostic command `portex`
echo -e "\nCreating symlink for portex command"
if [ ! -f "$INSTALL_DIR/portex" ]; then
  sudo ln -s "$INSTALL_DIR/$FILENAME" "$INSTALL_DIR/portex"
fi

# Set executable permissions
if ! chmod +x "$INSTALL_DIR/portex"; then
  echo "Failed to set executable permission on $INSTALL_DIR/portex" >&2
  exit 1
fi

echo "portex is successfully installed!"
exit 0