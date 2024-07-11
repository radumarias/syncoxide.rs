#!/bin/bash

set -e

export setup_base_path=/tmp/syncoxiders_test
export setup_num_paths=5
export setup_num_initial_files=0

BIN="$(dirname $0)/../../../../target/release/syncoxiders"

source ../common.sh
source ../setup.sh

for i in {1..10}; do
  size=$(generate_random_size 1 100)
  head -c $size </dev/urandom > $setup_base_path/path1/file$i
  touch $setup_base_path/path1/file$i
  for j in $(seq 2 $setup_num_paths); do
    cp $setup_base_path/path1/file$i $setup_base_path/path$j/file$i
    touch -r $setup_base_path/path1/file$i $setup_base_path/path$j/file$i
  done
done

# Run sync
$BIN --repo $setup_base_path/repo $setup_base_path/path1 $setup_base_path/path2

# Add files in path1
for i in {11..15}; do
  size=$(generate_random_size 1 100)
  head -c $size </dev/urandom > $setup_base_path/path1/file$i
  touch $setup_base_path/path1/file$i
done

# Change existing files in path1
for i in {1..5}; do
  size=$(generate_random_size 1 100)
  head -c $size </dev/urandom > $setup_base_path/path1/file$i
  touch $setup_base_path/path1/file$i
done

# Delete some files in path1
for i in {6..10}; do
  rm $setup_base_path/path1/file$i
done

# Run sync
$BIN --repo $setup_base_path/repo $setup_base_path/path1 $setup_base_path/path2 $setup_base_path/path3 $setup_base_path/path4 $setup_base_path/path5

# Verify new files in path2
for i in {11..15}; do
  for j in $(seq 2 $setup_num_paths); do
    if ! cmp -s $setup_base_path/path1/file$i $setup_base_path/path$j/file$i; then
      echo -e "${Red}file$i addition sync failed to path$j"
      exit 1
    fi
  done
done

# Verify modified files in path2
for i in {1..5}; do
  for j in $(seq 2 $setup_num_paths); do
    if ! cmp -s $setup_base_path/path1/file$i $setup_base_path/path$j/file$i; then
      echo -e "${Red}file$i modification sync failed to path$j"
      exit 1
    fi
  done
done

# Verify deleted files in path2
for i in {6..10}; do
  for j in $(seq 2 $setup_num_paths); do
    if [ -f $setup_base_path/path$j/file$i ]; then
      echo -e "${Red}file$i deletion sync failed to path$j"
      exit 1
    fi
  done
done

source ../cleanup.sh