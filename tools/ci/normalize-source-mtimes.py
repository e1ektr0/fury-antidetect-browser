#!/usr/bin/env python3
import os
import sys

def main():
    root_dir = sys.argv[1] if len(sys.argv) > 1 else 'core'
    # Use 2020-01-01 00:00:00 UTC as fixed past timestamp
    past_ts = 1577836800
    
    count = 0
    for root, dirs, files in os.walk(root_dir):
        # Exclude output directory and git metadata
        if 'out' in dirs:
            dirs.remove('out')
        if '.git' in dirs:
            dirs.remove('.git')
        for f in files:
            path = os.path.join(root, f)
            try:
                os.utime(path, (past_ts, past_ts))
                count += 1
            except OSError:
                pass

    print(f"Normalized timestamps of {count} source files to 2020-01-01 in {root_dir}")

if __name__ == '__main__':
    main()
