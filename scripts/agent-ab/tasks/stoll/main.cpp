#include <cstdio>
#include <fstream>
#include <string>

int main() {
  std::ifstream in("settings.txt");
  std::string line;
  long long timeout_ms = 0;
  unsigned long long limit = 0;
  while (std::getline(in, line)) {
    auto eq = line.find('=');
    auto key = line.substr(0, eq), value = line.substr(eq + 1);
    if (key == "timeout_s") timeout_ms = std::stoll(value) * 1000;
    if (key == "limit") limit = std::stoull(value);
  }
  std::printf("%lld\n", timeout_ms);
  return limit > 0 ? 0 : 1;
}
