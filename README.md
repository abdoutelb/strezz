This project is used to make a stress test to web sites
# Strezz

TO RUN the solution replace `URL` with your site build & run the container

*) `docker build -t stress --build-arg url=URL .`

*) `docker run -itd -p 4000:4000 -label stress:latest` 

check your `localhost:4000`

To run all tests run `jest` from CLI

HAVE FUN !!!
