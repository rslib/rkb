#include <cstdio>
#include <regex>
#include <string>

int main() {
  std::regex quoted(R"("(a|b)")");
  std::printf("%d\n", std::regex_match(std::string("\"a\""), quoted) ? 1 : 0);
}
