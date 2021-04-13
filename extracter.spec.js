const helpers = require("./extracter");

describe("extractor function", () => {
  it("should be defined", () => {
    expect(helpers.extractLinks).toBeDefined();
  });
  it("should throw error while empty params", async () => {
    expect(await helpers.extractLinks()).toEqual("urls not found");
  });
  it("should be a function", async() => {
    expect(await helpers.extractLinks("https://www.google.com/")).toBeInstanceOf(Array);
  });
});
