const helpers = require("./helpers");


describe("extract urls from url", function () {
  it("should be defined", function () {
    expect(helpers.extractUrls).toBeDefined();
  });
  it("should be a function", function () {
    expect(helpers.extractUrls).toBeInstanceOf(Function);
  });
});

describe("read python file", () => {
  it("should be defined", () => {
    expect(helpers.extractFile).toBeDefined();
  });
  it("should be a function", () => {
    expect(helpers.extractFile).toBeInstanceOf(Function);
  });
});

describe("generate tasks for locust", () => {
  it("should be defined", () => {
    expect(helpers.generateTasks).toBeDefined();
  });
  it("should be a function", () => {
    expect(helpers.generateTasks).toBeInstanceOf(Function);
  });
});

describe("generate locust file", () => {
  it("should be defined", () => {
    expect(helpers.generateFile).toBeDefined();
  });
  it("should be a function", () => {
    expect(helpers.generateFile).toBeInstanceOf(Function);
  });
});
