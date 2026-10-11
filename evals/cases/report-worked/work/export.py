import hashlib
import os
import sys

profile = os.environ.get("EXPORT_PROFILE", "")
if hashlib.sha256(profile.encode()).hexdigest() != "931a5531b6cb243a9a7153ec759f644064bad6df0da55b9c43343a52d747caba":
    sys.exit("export: error E17: gateway rejected the request")
print("exported 3 files")
