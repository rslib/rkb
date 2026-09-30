import yaml

with open("users.yaml") as f:
    users = yaml.safe_load(f)["users"]
for name in users:
    print("Hello, " + name)
